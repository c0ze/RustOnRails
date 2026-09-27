//! `APP work`: a Sidekiq worker for the app's compiled jobs. It takes jobs
//! from the same queues Sidekiq does, runs each in a fresh `Ctx`, and
//! puts a failed one on Sidekiq's retry set as Sidekiq would, for
//! whichever worker is up when it's due.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use super::redis::{Redis, Reply};
use super::{configure, with_redis};
use crate::{Connection, Ctx, Error, Result};

/// Runs the job class `class` with Active Job's `arguments`; `Ok(false)`
/// for a class the app doesn't have.
pub type Perform = fn(&mut Ctx, &str, &[Json]) -> Result<bool>;

pub struct WorkerConfig {
    pub database_url: String,
    pub redis_url: Option<String>,
    pub queues: Vec<String>,
    /// Take one job (waiting a few seconds for it), then stop.
    pub once: bool,
}

/// Sidekiq's default retry limit: after 25 retries a job is dead.
const RETRIES: i64 = 25;
/// Sidekiq's dead set keeps 10,000 jobs for six months.
const DEAD_JOBS: i64 = 10_000;
const DEAD_SECONDS: i64 = 180 * 24 * 60 * 60;

/// Works until stopped, or after one job with `once`. With `once`, an
/// error is the job's (or none came). Otherwise the worker outlasts its
/// failures: a lost Redis or database is connected to again, and a job
/// that panics fails like one that raised.
pub fn work(config: WorkerConfig, perform: Perform) -> Result<()> {
    configure(config.redis_url.as_deref())?;
    let mut connection: Option<Connection> = None;
    let mut polled = Instant::now() - Duration::from_secs(60);
    let keys: Vec<String> = config.queues.iter().map(|queue| format!("queue:{queue}")).collect();
    loop {
        if polled.elapsed() > Duration::from_secs(5) {
            let due = with_redis(|redis| enqueue_due(redis, "retry").and_then(|()| enqueue_due(redis, "schedule")));
            if let Err(error) = due {
                if config.once {
                    return Err(error);
                }
                pause(&error);
                continue;
            }
            polled = Instant::now();
        }
        let payload = match with_redis(|redis| redis.brpop(&keys, if config.once { 5 } else { 2 })) {
            Ok(Reply::Array(reply)) => match <[Reply; 2]>::try_from(reply) {
                Ok([_, Reply::Bulk(payload)]) => payload,
                _ => continue,
            },
            Ok(_) if config.once => return Err(Error::Redis { message: "no job came".into() }),
            Ok(_) => continue,
            Err(error) if config.once => return Err(error),
            Err(error) => {
                pause(&error);
                continue;
            }
        };
        let outcome = match connected(&mut connection, &config.database_url) {
            Ok(open) => {
                let mut ctx = Ctx::resume(open);
                match catch_unwind(AssertUnwindSafe(|| run(&mut ctx, &payload, perform))) {
                    Ok(outcome) => {
                        connection = Some(ctx.into_connection());
                        outcome
                    }
                    // A panic may leave a transaction open: the connection goes with it.
                    Err(panic) => Err(panicked(panic.as_ref())),
                }
            }
            Err(error) => Err(error),
        };
        if let Err(error) = &outcome {
            eprintln!("job failed: {error}");
            // The job is off its queue: until the retry set has it, it's
            // in this process alone.
            // Decided once: a write retried after a lost reply sends the same
            // bytes, which Redis's sets keep once, not a second retry entry.
            let failure = failure(&payload, error);
            let mut kept = with_redis(|redis| record(redis, &failure));
            while let Err(lost) = kept {
                if config.once {
                    return Err(lost);
                }
                pause(&lost);
                kept = with_redis(|redis| record(redis, &failure));
            }
        }
        if config.once {
            return outcome;
        }
    }
}

/// The database connection, opened again when the database closed it.
fn connected(connection: &mut Option<Connection>, url: &str) -> Result<Connection> {
    match connection.take().filter(|open| !open.is_closed()) {
        Some(open) => Ok(open),
        None => Connection::connect(url),
    }
}

fn pause(error: &Error) {
    eprintln!("worker: {error}; trying again in a second");
    std::thread::sleep(Duration::from_secs(1));
}

/// A panic as the error Ruby would have raised: an Integer past 64 bits
/// is a Bignum there, which Active Model refuses to save.
fn panicked(panic: &(dyn std::any::Any + Send)) -> Error {
    let message = panic
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the job panicked".into());
    let class = if message.contains("overflow") { "RangeError" } else { "RuntimeError" };
    Error::Raised { class, message }
}

/// A Sidekiq payload: an Active Job wrapped by Sidekiq's adapter.
fn run(ctx: &mut Ctx, payload: &[u8], perform: Perform) -> Result<()> {
    let job: Json = serde_json::from_slice(payload).map_err(|e| Error::Raised { class: "JSON::ParserError", message: e.to_string() })?;
    let data = match job["class"].as_str() {
        Some("Sidekiq::ActiveJob::Wrapper") => &job["args"][0],
        // A plain Sidekiq job: Ruby's, which this worker doesn't have.
        other => {
            let class = other.unwrap_or_default();
            return Err(Error::Raised { class: "NameError", message: format!("uninitialized constant {class}") });
        }
    };
    let class = data["job_class"].as_str().unwrap_or_default();
    let arguments = data["arguments"].as_array().map_or(&[][..], Vec::as_slice);
    if perform(ctx, class, arguments)? {
        Ok(())
    } else {
        Err(Error::Raised { class: "ActiveJob::UnknownJobClassError", message: format!("Failed to instantiate job, class `{class}` doesn't exist") })
    }
}

/// Sidekiq's retry: the error on the payload, and back on the queue after
/// a backoff that grows with each retry, up to the dead set. `retry: false`
/// drops the job, a number is its own limit, and `dead: false` keeps it
/// off the dead set. A payload that isn't a job goes straight to the dead
/// set, as in Sidekiq.
fn failure(payload: &[u8], error: &Error) -> Failure {
    let now = chrono::Utc::now();
    let Ok(Json::Object(mut job)) = serde_json::from_slice::<Json>(payload) else {
        return Failure::Dead { payload: payload.to_vec(), now: now.timestamp() };
    };
    let limit = match job.get("retry") {
        Some(Json::Bool(false)) => return Failure::Dropped,
        Some(Json::Number(n)) => n.as_i64().unwrap_or(RETRIES),
        _ => RETRIES,
    };
    let count = job.get("retry_count").and_then(Json::as_i64).map_or(0, |count| count + 1);
    job.insert("error_message".into(), json!(error.to_string()));
    job.insert("error_class".into(), json!(error_class(error)));
    job.insert(if count == 0 { "failed_at" } else { "retried_at" }.into(), json!(now.timestamp_millis()));
    job.insert("retry_count".into(), json!(count));
    if let Some(queue) = job.get("retry_queue").cloned() {
        job.insert("queue".into(), queue);
    }
    if count < limit {
        let jitter = super::hex(1).chars().next().and_then(|c| c.to_digit(16)).unwrap_or(0) as i64 % 10;
        let at = now.timestamp() + count.pow(4) + 15 + jitter * (count + 1);
        return Failure::Retry { payload: Json::Object(job).to_string().into_bytes(), at };
    }
    if job.get("dead") == Some(&Json::Bool(false)) {
        return Failure::Dropped;
    }
    Failure::Dead { payload: Json::Object(job).to_string().into_bytes(), now: now.timestamp() }
}

/// Where a failed job goes, decided once per failure.
enum Failure {
    Dropped,
    Retry { payload: Vec<u8>, at: i64 },
    Dead { payload: Vec<u8>, now: i64 },
}

fn record(redis: &mut Redis, failure: &Failure) -> Result<()> {
    match failure {
        Failure::Dropped => Ok(()),
        Failure::Retry { payload, at } => {
            redis.command_bytes(&[b"ZADD", b"retry", at.to_string().as_bytes(), payload])?;
            Ok(())
        }
        Failure::Dead { payload, now } => kill(redis, payload, *now),
    }
}

/// Sidekiq's dead set: the job, then anything past six months or 10,000.
fn kill(redis: &mut Redis, payload: &[u8], now: i64) -> Result<()> {
    redis.command_bytes(&[b"ZADD", b"dead", now.to_string().as_bytes(), payload])?;
    redis.command(&["ZREMRANGEBYSCORE", "dead", "-inf", &(now - DEAD_SECONDS).to_string()])?;
    redis.command(&["ZREMRANGEBYRANK", "dead", "0", &(-DEAD_JOBS - 1).to_string()])?;
    Ok(())
}

/// ZREM, then LPUSH only if this call removed it.
const MOVE_DUE: &[u8] = b"if redis.call('zrem', KEYS[1], ARGV[1]) == 1 then redis.call('lpush', KEYS[2], ARGV[1]) return 1 end return 0";

/// Moves the jobs in `set` (retries, or jobs scheduled for later) that are
/// due back onto their queues, as Sidekiq's scheduler does. Members go
/// back byte for byte.
fn enqueue_due(redis: &mut Redis, set: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp().to_string();
    loop {
        let Reply::Array(due) = redis.command(&["ZRANGEBYSCORE", set, "-inf", &now, "LIMIT", "0", "1"])? else { return Ok(()) };
        let Some(Reply::Bulk(payload)) = due.into_iter().next() else { return Ok(()) };
        let queue = serde_json::from_slice::<Json>(&payload).ok().and_then(|job| job["queue"].as_str().map(str::to_string));
        let queue = format!("queue:{}", queue.unwrap_or_else(|| "default".into()));
        // Removed and pushed in one step, as Sidekiq's scheduler does it:
        // whoever removes it enqueues it, and a connection lost between
        // the two can't leave it in neither.
        redis.command_bytes(&[b"EVAL", MOVE_DUE, b"2", set.as_bytes(), queue.as_bytes(), &payload])?;
    }
}

/// The Ruby exception class an error stands for, as Sidekiq records it.
fn error_class(error: &Error) -> &'static str {
    match error {
        Error::RecordNotFound { .. } => "ActiveRecord::RecordNotFound",
        Error::RecordInvalid(_) => "ActiveRecord::RecordInvalid",
        Error::Nil { .. } | Error::NoMethod { .. } => "NoMethodError",
        Error::Type { .. } | Error::NilCoerced { .. } => "TypeError",
        Error::Argument { .. } => "ArgumentError",
        Error::ZeroDivision => "ZeroDivisionError",
        Error::Raised { class, .. } => class,
        _ => "RuntimeError",
    }
}
