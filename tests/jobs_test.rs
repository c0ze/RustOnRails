//! Active Job on Sidekiq: the payload Sidekiq's adapter pushes, the
//! arguments Active Job serializes, and a worker's runs and retries. Each
//! test has queues of its own on the test Redis.

mod support;

use std::sync::Mutex;

use rustonrails::jobs::redis::{Redis, Reply};
use rustonrails::jobs::{self, Job, JobApp, WorkerConfig};
use rustonrails::{Behavior, Ctx, Error, Json, Model, Result, Time, json, model, now};

model! {
    pub struct Author in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Author {
    fn behavior() -> &'static Behavior<Self> {
        static B: std::sync::LazyLock<Behavior<Author>> = std::sync::LazyLock::new(Behavior::new);
        &B
    }
}

const APP: JobApp = JobApp { name: "blog", locale: "en", timezone: "UTC" };

fn redis_url() -> String {
    std::env::var("RUSTONRAILS_TEST_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:54379/15".into())
}

/// The test Redis, with `queues` emptied.
fn redis(queues: &[&str]) -> Redis {
    jobs::configure(Some(&redis_url())).unwrap();
    let mut redis = Redis::connect(&redis_url()).expect("the test Redis (rake redis:start in Rutile) is up");
    for queue in queues {
        redis.command(&["DEL", &format!("queue:{queue}")]).unwrap();
    }
    redis
}

fn pop(redis: &mut Redis, queue: &str) -> Json {
    match redis.command(&["RPOP", &format!("queue:{queue}")]).unwrap() {
        Reply::Bulk(payload) => serde_json::from_slice(&payload).unwrap(),
        other => panic!("nothing on queue:{queue}: {other:?}"),
    }
}

fn keys(json: &Json) -> Vec<&str> {
    json.as_object().unwrap().keys().map(String::as_str).collect()
}

/// What Sidekiq 8's Active Job adapter pushes, key for key.
#[test]
fn test_perform_later_pushes_sidekiqs_payload() {
    let mut redis = redis(&["payload"]);
    let job = Job { class: "RestockJob", queue: "payload" };
    job.perform_later(&APP, vec![json!({ "_aj_globalid": "gid://blog/Author/1" }), json!(3)]).unwrap();
    let payload = pop(&mut redis, "payload");
    assert_eq!(vec!["retry", "queue", "wrapped", "args", "class", "jid", "created_at", "enqueued_at"], keys(&payload));
    assert_eq!(
        (json!(true), json!("payload"), json!("RestockJob"), json!("Sidekiq::ActiveJob::Wrapper")),
        (payload["retry"].clone(), payload["queue"].clone(), payload["wrapped"].clone(), payload["class"].clone())
    );
    let jid = payload["jid"].as_str().unwrap();
    assert!(jid.len() == 24 && jid.chars().all(|c| c.is_ascii_hexdigit()), "{jid}");
    assert_eq!(payload["created_at"], payload["enqueued_at"]);
    assert!(payload["enqueued_at"].as_i64().unwrap() > 1_700_000_000_000, "milliseconds");

    let data = &payload["args"][0];
    let expected = [
        "job_class", "job_id", "provider_job_id", "queue_name", "priority", "arguments", "executions",
        "exception_executions", "locale", "timezone", "enqueued_at", "scheduled_at",
    ];
    assert_eq!(expected.to_vec(), keys(data));
    assert_eq!(json!([{ "_aj_globalid": "gid://blog/Author/1" }, 3]), data["arguments"]);
    assert_eq!((json!("payload"), json!(0), json!({})), (data["queue_name"].clone(), data["executions"].clone(), data["exception_executions"].clone()));
    assert_eq!((json!(null), json!(null), json!(null)), (data["provider_job_id"].clone(), data["priority"].clone(), data["scheduled_at"].clone()));
    assert_eq!((json!("en"), json!("UTC")), (data["locale"].clone(), data["timezone"].clone()));
    let id = data["job_id"].as_str().unwrap();
    assert_eq!(vec![8, 4, 4, 4, 12], id.split('-').map(str::len).collect::<Vec<_>>());
    assert_eq!(Some('4'), id.chars().nth(14), "a version 4 UUID");
    let at = data["enqueued_at"].as_str().unwrap();
    assert!(at.len() == 30 && at.ends_with('Z') && at.as_bytes()[19] == b'.', "nanoseconds, as Active Job writes them: {at}");
    match redis.command(&["SISMEMBER", "queues", "payload"]).unwrap() {
        Reply::Int(1) => {}
        other => panic!("the queue isn't registered: {other:?}"),
    }
}

/// A record goes by its GlobalID and comes back by it; nil stays nil.
#[test]
fn test_record_arguments() {
    let mut ctx = support::ctx();
    let record = Author {
        name: Some("ann".into()), email: Some("jobs@example.com".into()), created_at: Some(now()), updated_at: Some(now()),
        ..Author::default()
    };
    let id = Author::insert(&mut ctx, record).unwrap();
    let author = Author::find(&mut ctx, id).unwrap();
    let argument = jobs::record_argument(&ctx, &APP, author).unwrap();
    assert_eq!(json!({ "_aj_globalid": format!("gid://blog/Author/{id}") }), argument);
    assert_eq!(Json::Null, jobs::record_argument::<Author>(&ctx, &APP, None).unwrap());

    let found = jobs::record_at::<Author>(&mut ctx, &[json!(1), argument], 1).unwrap();
    assert_eq!(Some(id), ctx[found].id);
    // GlobalID's default locator finds the record whichever app it names.
    let other_app = json!({ "_aj_globalid": format!("gid://store/Author/{id}") });
    let again = jobs::record_at::<Author>(&mut ctx, &[other_app], 0).unwrap();
    assert_eq!(Some(id), ctx[again].id);

    let failed = |ctx: &mut Ctx, argument: Json| match jobs::record_at::<Author>(ctx, &[argument], 0) {
        Err(Error::Raised { class, message }) => format!("{class}: {message}"),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        "ActiveJob::DeserializationError: Error while trying to deserialize arguments: Couldn't find Author with 'id'=\"0\"",
        failed(&mut ctx, json!({ "_aj_globalid": "gid://blog/Author/0" }))
    );
    assert_eq!(
        "ActiveJob::DeserializationError: Error while trying to deserialize arguments: argument 0 isn't a Author",
        failed(&mut ctx, json!({ "_aj_globalid": format!("gid://blog/Post/{id}") }))
    );
    assert!(failed(&mut ctx, Json::Null).starts_with("ActiveJob::DeserializationError: "));

    // Rails won't serialize a record that was never saved.
    let unsaved = ctx.build(Author::default());
    match jobs::record_argument(&ctx, &APP, unsaved) {
        Err(Error::Raised { class, message }) => assert_eq!(
            ("ActiveJob::SerializationError", "Unable to serialize Author without an id. (Maybe you forgot to call save?)"),
            (class, message.as_str())
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_scalar_arguments() {
    let arguments = [json!(3), json!(2.5), json!("a"), json!(true), Json::Null];
    assert_eq!(3, jobs::scalar_at::<i64>(&arguments, 0).unwrap());
    assert_eq!(2.5, jobs::scalar_at::<f64>(&arguments, 1).unwrap());
    assert_eq!("a", jobs::scalar_at::<String>(&arguments, 2).unwrap());
    assert!(jobs::scalar_at::<bool>(&arguments, 3).unwrap());
    assert_eq!(None, jobs::scalar_at::<Option<i64>>(&arguments, 4).unwrap());
    assert_eq!(None, jobs::scalar_at::<Option<i64>>(&arguments, 9).unwrap(), "an argument the job wasn't given is nil");
    assert!(jobs::scalar_at::<i64>(&arguments, 2).is_err());
    assert!(jobs::scalar_at::<i64>(&arguments, 4).is_err());
}

static RAN: Mutex<Vec<(String, Json)>> = Mutex::new(Vec::new());

/// The worker's app: "Recorder" notes what it ran with, "Failing" raises,
/// and there's no other job.
fn perform(_ctx: &mut Ctx, class: &str, arguments: &[Json]) -> Result<bool> {
    match class {
        "Recorder" => {
            RAN.lock().unwrap().push((class.to_string(), Json::Array(arguments.to_vec())));
            Ok(true)
        }
        "Failing" => Err(Error::Argument { message: "wrong number of arguments (given 0, expected 1)".into() }),
        "Panicking" => panic!("attempt to add with overflow"),
        _ => Ok(false),
    }
}

fn work_once(queue: &str) -> Result<()> {
    support::prepare();
    let config = WorkerConfig { database_url: support::url(), redis_url: Some(redis_url()), queues: vec![queue.into()], once: true };
    jobs::work(config, perform)
}

fn ran(marker: &str) -> bool {
    RAN.lock().unwrap().iter().any(|(_, arguments)| arguments[0] == marker)
}

#[test]
fn test_the_worker_runs_a_job() {
    let mut redis = redis(&["work"]);
    Job { class: "Recorder", queue: "work" }.perform_later(&APP, vec![json!("ran"), json!(1)]).unwrap();
    work_once("work").unwrap();
    assert!(ran("ran"));
    assert_eq!(Reply::Int(0), redis.command(&["LLEN", "queue:work"]).unwrap());
}

/// Sidekiq's retry: the error on the job, onto the retry set, due after
/// the first backoff (15 to 24 seconds).
#[test]
fn test_a_failed_job_goes_on_the_retry_set() {
    let mut redis = redis(&["failing", "unknown"]);
    Job { class: "Failing", queue: "failing" }.perform_later(&APP, vec![]).unwrap();
    assert!(matches!(work_once("failing"), Err(Error::Argument { .. })));
    Job { class: "Nowhere", queue: "unknown" }.perform_later(&APP, vec![]).unwrap();
    assert!(matches!(work_once("unknown"), Err(Error::Raised { class: "ActiveJob::UnknownJobClassError", .. })));

    let Reply::Array(entries) = redis.command(&["ZRANGE", "retry", "0", "-1", "WITHSCORES"]).unwrap() else { panic!() };
    let now = chrono::Utc::now().timestamp();
    let mut found = Vec::new();
    for pair in entries.chunks(2) {
        let [Reply::Bulk(payload), Reply::Bulk(score)] = pair else { panic!("{pair:?}") };
        let job: Json = serde_json::from_slice(payload).unwrap();
        if !matches!(job["queue"].as_str(), Some("failing" | "unknown")) {
            continue;
        }
        redis.command(&["ZREM", "retry", std::str::from_utf8(payload).unwrap()]).unwrap();
        let due = std::str::from_utf8(score).unwrap().parse::<i64>().unwrap() - now;
        assert!((14..=25).contains(&due), "due in {due}s");
        assert_eq!(json!(0), job["retry_count"]);
        assert!(job["failed_at"].as_i64().is_some() && job.get("retried_at").is_none());
        found.push((job["error_class"].as_str().unwrap().to_string(), job["error_message"].as_str().unwrap().to_string()));
    }
    found.sort();
    assert_eq!(
        vec![
            ("ActiveJob::UnknownJobClassError".to_string(), "Failed to instantiate job, class `Nowhere` doesn't exist".to_string()),
            ("ArgumentError".to_string(), "wrong number of arguments (given 0, expected 1)".to_string()),
        ],
        found
    );
}

/// A retry that's due goes back on its queue, and runs again; one that has
/// used up its retries goes to the dead set instead.
#[test]
fn test_retries_come_due_and_die() {
    let mut redis = redis(&["due", "dying"]);
    let due = json!({ "retry": true, "queue": "due", "class": "Sidekiq::ActiveJob::Wrapper", "jid": "due",
                      "args": [{ "job_class": "Recorder", "arguments": ["due again"] }], "retry_count": 0 });
    redis.command(&["ZADD", "retry", &(chrono::Utc::now().timestamp() - 1).to_string(), &due.to_string()]).unwrap();
    work_once("due").unwrap();
    assert!(ran("due again"));

    let dying = json!({ "retry": true, "queue": "dying", "class": "Sidekiq::ActiveJob::Wrapper", "jid": "dying",
                        "args": [{ "job_class": "Failing", "arguments": [] }], "retry_count": 24 });
    redis.command(&["LPUSH", "queue:dying", &dying.to_string()]).unwrap();
    assert!(work_once("dying").is_err());
    let Reply::Array(dead) = redis.command(&["ZRANGE", "dead", "0", "-1"]).unwrap() else { panic!() };
    let dead: Vec<Json> = dead.iter().map(|entry| match entry {
        Reply::Bulk(payload) => serde_json::from_slice(payload).unwrap(),
        other => panic!("{other:?}"),
    }).collect();
    let job = dead.iter().find(|job| job["jid"] == "dying").expect("on the dead set");
    assert_eq!(json!(25), job["retry_count"]);
    assert!(job["retried_at"].as_i64().is_some());
    redis.command(&["ZREM", "dead", &job.to_string()]).unwrap();

    // What isn't JSON can't be retried; Sidekiq buries it.
    redis.command(&["LPUSH", "queue:dying", "{not json"]).unwrap();
    assert!(matches!(work_once("dying"), Err(Error::Raised { class: "JSON::ParserError", .. })));
    assert_eq!(Reply::Int(1), redis.command(&["ZREM", "dead", "{not json"]).unwrap());
}

/// A payload as Sidekiq keeps it, for `class` on `queue`.
fn payload(class: &str, queue: &str, extra: Json) -> Json {
    let mut job = json!({ "retry": true, "queue": queue, "class": "Sidekiq::ActiveJob::Wrapper", "jid": format!("{class}-{queue}"),
                          "args": [{ "job_class": class, "arguments": [queue] }] });
    job.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    job
}

/// Every entry of the sorted set `set` for `queue`, removed.
fn take(redis: &mut Redis, set: &str, queue: &str) -> Vec<Json> {
    let Reply::Array(entries) = redis.command(&["ZRANGE", set, "0", "-1"]).unwrap() else { panic!() };
    let mut found = Vec::new();
    for entry in entries {
        let Reply::Bulk(bytes) = entry else { panic!() };
        let Ok(job) = serde_json::from_slice::<Json>(&bytes) else { continue };
        if job["queue"] == queue || job["retry_queue"] == queue || job["args"][0]["arguments"][0] == queue {
            redis.command_bytes(&[b"ZREM", set.as_bytes(), &bytes]).unwrap();
            found.push(job);
        }
    }
    found
}

/// A job that panics fails like one that raised: onto the retry set,
/// and the worker lives on.
#[test]
fn test_a_panic_is_a_failure() {
    let mut redis = redis(&["panicking"]);
    redis.command(&["LPUSH", "queue:panicking", &payload("Panicking", "panicking", json!({})).to_string()]).unwrap();
    match work_once("panicking") {
        Err(Error::Raised { class: "RangeError", message }) => assert_eq!("attempt to add with overflow", message),
        other => panic!("{other:?}"),
    }
    let retried = take(&mut redis, "retry", "panicking");
    assert_eq!(1, retried.len());
    assert_eq!("RangeError", retried[0]["error_class"]);
}

/// Sidekiq's retry option: false drops the job, a number is its limit,
/// and `dead: false` keeps it off the dead set; `retry_queue` is where
/// its retries go.
#[test]
fn test_sidekiqs_retry_options() {
    let mut redis = redis(&["dropped", "limited", "undying", "requeued"]);
    let jobs = [
        ("dropped", json!({ "retry": false })),
        ("limited", json!({ "retry": 2, "retry_count": 1 })),
        ("undying", json!({ "retry": 0, "dead": false })),
        ("requeued", json!({ "retry_queue": "later" })),
    ];
    for (queue, extra) in &jobs {
        redis.command(&["LPUSH", &format!("queue:{queue}"), &payload("Failing", queue, extra.clone()).to_string()]).unwrap();
        assert!(work_once(queue).is_err());
    }
    let mut sets = |queue: &str| (take(&mut redis, "retry", queue), take(&mut redis, "dead", queue));
    let (retried, dead) = sets("dropped");
    assert_eq!((0, 0), (retried.len(), dead.len()));
    let (retried, dead) = sets("limited");
    assert_eq!((0, 1), (retried.len(), dead.len()));
    assert_eq!(json!(2), dead[0]["retry_count"]);
    let (retried, dead) = sets("undying");
    assert_eq!((0, 0), (retried.len(), dead.len()));
    let (retried, _) = sets("requeued");
    assert_eq!(1, retried.len());
    assert_eq!("later", retried[0]["queue"]);
    redis.command(&["DEL", "queue:later"]).unwrap();
}

/// Jobs Rails schedules (`set(wait:)`) run when they're due.
#[test]
fn test_scheduled_jobs_run_when_due() {
    let mut redis = redis(&["scheduled"]);
    let past = (chrono::Utc::now().timestamp() - 1).to_string();
    redis.command(&["ZADD", "schedule", &past, &payload("Recorder", "scheduled", json!({})).to_string()]).unwrap();
    work_once("scheduled").unwrap();
    assert!(ran("scheduled"));
}

/// A member that isn't UTF-8 moves like any other: nothing spins on it.
#[test]
fn test_members_move_byte_for_byte() {
    let mut redis = redis(&["bytes"]);
    let past = (chrono::Utc::now().timestamp() - 1).to_string();
    let member: &[u8] = b"\xff{not a job}";
    redis.command_bytes(&[b"ZADD", b"retry", past.as_bytes(), member]).unwrap();
    // It has no queue, so it lands on the default one.
    let _ = work_once("bytes");
    assert_eq!(Reply::Int(1), redis.command_bytes(&[b"LREM", b"queue:default", b"0", member]).unwrap());
}

#[test]
fn test_float_and_arity_checks() {
    assert_eq!(json!(2.5), jobs::float_argument(2.5).unwrap());
    assert_eq!(Json::Null, jobs::float_argument(None).unwrap());
    match jobs::float_argument(f64::NAN) {
        Err(Error::Raised { class: "JSON::GeneratorError", message }) => assert_eq!("NaN not allowed in JSON", message),
        other => panic!("{other:?}"),
    }
    assert!(jobs::arity(&[json!(1)], 1).is_ok());
    assert_eq!("wrong number of arguments (given 2, expected 1)", jobs::arity(&[json!(1), json!(2)], 1).unwrap_err().to_string());
}

#[test]
fn test_only_redis_urls() {
    let error = jobs::configure(Some("rediss://example.com:6380")).unwrap_err();
    assert_eq!("REDIS_URL: rediss:// isn't supported, only redis://", error.to_string());
}
