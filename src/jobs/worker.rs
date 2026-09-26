//! `APP work`: a Sidekiq worker for the app's compiled jobs. It takes jobs
//! from the same queues Sidekiq does, runs each in a fresh `Ctx`, and
//! puts a failed one on Sidekiq's retry set as Sidekiq would, for
//! whichever worker is up when it's due.

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

/// Sidekiq's retry limit: after 25 retries a job is dead.
const RETRIES: i64 = 25;

/// Works until stopped, or after one job with `once`. With `once`, an
/// error is the job's (or none came).
pub fn work(config: WorkerConfig, perform: Perform) -> Result<()> {
    configure(config.redis_url.as_deref());
    let mut connection = Some(Connection::connect(&config.database_url)?);
    let mut polled = Instant::now() - Duration::from_secs(60);
    let mut keys: Vec<String> = config.queues.iter().map(|queue| format!("queue:{queue}")).collect();
    keys.push(if config.once { "5" } else { "2" }.to_string());
    loop {
        if polled.elapsed() > Duration::from_secs(5) {
            with_redis(|redis| enqueue_due(redis, "retry"))?;
            polled = Instant::now();
        }
        let args: Vec<&str> = std::iter::once("BRPOP").chain(keys.iter().map(String::as_str)).collect();
        let payload = match with_redis(|redis| redis.command(&args))? {
            Reply::Array(reply) => match reply.as_slice() {
                [_, Reply::Bulk(payload)] => String::from_utf8_lossy(payload).into_owned(),
                _ => continue,
            },
            _ if config.once => return Err(Error::Redis { message: "no job came".into() }),
            _ => continue,
        };
        let mut ctx = Ctx::resume(connection.take().ok_or(Error::Redis { message: "lost the database".into() })?);
        let outcome = run(&mut ctx, &payload, perform);
        connection = Some(ctx.into_connection());
        if let Err(error) = &outcome {
            eprintln!("job failed: {error}");
            with_redis(|redis| retry(redis, &payload, error))?;
        }
        if config.once {
            return outcome;
        }
    }
}

/// A Sidekiq payload: an Active Job wrapped by Sidekiq's adapter.
fn run(ctx: &mut Ctx, payload: &str, perform: Perform) -> Result<()> {
    let job: Json = serde_json::from_str(payload).map_err(|e| Error::Raised { class: "JSON::ParserError", message: e.to_string() })?;
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
/// a backoff that grows with each retry, up to the dead set. A payload
/// that isn't a job goes straight to the dead set, as in Sidekiq.
fn retry(redis: &mut Redis, payload: &str, error: &Error) -> Result<()> {
    let now = chrono::Utc::now();
    let Ok(Json::Object(mut job)) = serde_json::from_str::<Json>(payload) else {
        redis.command(&["ZADD", "dead", &now.timestamp().to_string(), payload])?;
        return Ok(());
    };
    let count = job.get("retry_count").and_then(Json::as_i64).map_or(0, |count| count + 1);
    job.insert("error_message".into(), json!(error.to_string()));
    job.insert("error_class".into(), json!(error_class(error)));
    job.insert(if count == 0 { "failed_at" } else { "retried_at" }.into(), json!(now.timestamp_millis()));
    job.insert("retry_count".into(), json!(count));
    let retrying = job.get("retry").is_some_and(|retry| retry == &Json::Bool(true)) && count < RETRIES;
    let (set, at) = if retrying {
        let jitter = super::hex(1).chars().next().and_then(|c| c.to_digit(16)).unwrap_or(0) as i64 % 10;
        ("retry", now.timestamp() + count.pow(4) + 15 + jitter * (count + 1))
    } else {
        ("dead", now.timestamp())
    };
    redis.command(&["ZADD", set, &at.to_string(), &Json::Object(job).to_string()])?;
    Ok(())
}

/// Moves the jobs in `set` that are due back onto their queues, as
/// Sidekiq's scheduler does.
fn enqueue_due(redis: &mut Redis, set: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp().to_string();
    loop {
        let Reply::Array(due) = redis.command(&["ZRANGEBYSCORE", set, "-inf", &now, "LIMIT", "0", "1"])? else { return Ok(()) };
        let Some(Reply::Bulk(payload)) = due.into_iter().next() else { return Ok(()) };
        let payload = String::from_utf8_lossy(&payload).into_owned();
        // Whoever removes it enqueues it, so two schedulers can't both.
        if redis.command(&["ZREM", set, &payload])? != Reply::Int(1) {
            continue;
        }
        let queue = serde_json::from_str::<Json>(&payload).ok().and_then(|job| job["queue"].as_str().map(str::to_string));
        let queue = format!("queue:{}", queue.unwrap_or_else(|| "default".into()));
        redis.command(&["LPUSH", &queue, &payload])?;
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
