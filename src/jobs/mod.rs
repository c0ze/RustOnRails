//! Active Job on Sidekiq, in Sidekiq's own format: a job enqueued here is
//! one Sidekiq's Ruby workers run, and a Ruby app's job is one this
//! worker runs. Both share the Redis queues.

pub mod redis;
mod worker;

use std::cell::RefCell;
use std::sync::OnceLock;

use serde_json::{Value as Json, json};

use crate::{Ctx, Error, Handle, Model, Result};
use redis::Redis;
pub use worker::{WorkerConfig, work};

/// The app's `GlobalID.app` and the locale and time zone Active Job
/// records with each job.
#[derive(Clone, Copy, Debug)]
pub struct JobApp {
    pub name: &'static str,
    pub locale: &'static str,
    pub timezone: &'static str,
}

/// A job class: `class RestockJob < ApplicationJob; queue_as :default`.
#[derive(Clone, Copy, Debug)]
pub struct Job {
    pub class: &'static str,
    pub queue: &'static str,
}

static REDIS_URL: OnceLock<String> = OnceLock::new();

thread_local! {
    static CONNECTION: RefCell<Option<Redis>> = const { RefCell::new(None) };
}

/// Where the queues are: Sidekiq's `REDIS_URL`, its default otherwise.
/// Only `redis://` is spoken here; anything else fails at startup rather
/// than at the first job.
pub fn configure(redis_url: Option<&str>) -> Result<()> {
    let url = redis_url.unwrap_or("redis://localhost:6379/0");
    if !url.starts_with("redis://") {
        let scheme = url.split("://").next().unwrap_or(url);
        return Err(Error::Redis { message: format!("REDIS_URL: {scheme}:// isn't supported, only redis://") });
    }
    REDIS_URL.get_or_init(|| url.to_string());
    Ok(())
}

/// Runs `f` on this thread's Redis connection, connecting the first time.
/// A connection that fails is dropped and `f` runs once more on a new
/// one, as Sidekiq's client reconnects; an error Redis answers is final.
pub(crate) fn with_redis<T>(mut f: impl FnMut(&mut Redis) -> Result<T>) -> Result<T> {
    CONNECTION.with(|cell| {
        let mut connection = cell.borrow_mut();
        let url = REDIS_URL.get().map_or("redis://localhost:6379/0", String::as_str);
        let fresh = connection.is_none();
        let redis = match connection.as_mut() {
            Some(redis) => redis,
            None => connection.insert(Redis::connect(url)?),
        };
        match f(redis) {
            Err(_) if redis.is_broken() && !fresh => {
                *connection = None;
                let redis = connection.insert(Redis::connect(url)?);
                let outcome = f(redis);
                if redis.is_broken() {
                    *connection = None;
                }
                outcome
            }
            outcome => {
                if redis.is_broken() {
                    *connection = None;
                }
                outcome
            }
        }
    })
}

impl Job {
    /// `perform_later(*arguments)`: the job as Sidekiq's Active Job adapter
    /// pushes it, onto the job's queue.
    pub fn perform_later(&self, app: &JobApp, arguments: Vec<Json>) -> Result<()> {
        let now = chrono::Utc::now();
        let millis = now.timestamp_millis();
        let payload = json!({
            "retry": true,
            "queue": self.queue,
            "wrapped": self.class,
            "args": [{
                "job_class": self.class,
                "job_id": uuid(),
                "provider_job_id": null,
                "queue_name": self.queue,
                "priority": null,
                "arguments": arguments,
                "executions": 0,
                "exception_executions": {},
                "locale": app.locale,
                "timezone": app.timezone,
                "enqueued_at": now.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string(),
                "scheduled_at": null,
            }],
            "class": "Sidekiq::ActiveJob::Wrapper",
            "jid": hex(12),
            "created_at": millis,
            "enqueued_at": millis,
        });
        let queue = format!("queue:{}", self.queue);
        with_redis(|redis| {
            redis.command(&["SADD", "queues", self.queue])?;
            redis.command(&["LPUSH", &queue, &payload.to_string()])?;
            Ok(())
        })
    }
}

/// A record as an Active Job argument: its GlobalID, which the other side
/// finds again; nil as nil. Rails raises on a record that was never saved.
pub fn record_argument<M: Model>(ctx: &Ctx, app: &JobApp, record: impl Into<Option<Handle<M>>>) -> Result<Json> {
    let Some(record) = record.into() else { return Ok(Json::Null) };
    let id = ctx[record].id().ok_or_else(|| Error::Raised {
        class: "ActiveJob::SerializationError",
        message: format!("Unable to serialize {} without an id. (Maybe you forgot to call save?)", M::NAME),
    })?;
    Ok(json!({ "_aj_globalid": format!("gid://{}/{}/{id}", app.name, M::NAME) }))
}

/// The record the argument at `index` names by GlobalID. As GlobalID's
/// default locator does, it finds the model's record by id whichever app
/// the id names.
pub fn record_at<M: Model>(ctx: &mut Ctx, arguments: &[Json], index: usize) -> Result<Handle<M>> {
    let gid = arguments.get(index).and_then(|argument| argument.get("_aj_globalid")).and_then(Json::as_str);
    let id = gid.and_then(|gid| gid.strip_prefix("gid://")).and_then(|rest| {
        let (_app, rest) = rest.split_once('/')?;
        let (model, id) = rest.split_once('/')?;
        (model == M::NAME).then_some(id)
    });
    let Some(id) = id else { return Err(deserialization(format!("argument {index} isn't a {}", M::NAME))) };
    M::find(ctx, id).map_err(|error| deserialization(error.to_string()))
}

/// A Float argument: Active Job's JSON has no NaN or Infinity, so Rails
/// raises generating it.
pub fn float_argument(value: impl Into<Option<f64>>) -> Result<Json> {
    match value.into() {
        Some(f) if !f.is_finite() => Err(Error::Raised { class: "JSON::GeneratorError", message: format!("{} not allowed in JSON", crate::Value::Float(f).to_s()) }),
        other => Ok(Json::from(other)),
    }
}

/// `perform` takes exactly `count` arguments, as Ruby checks a method's.
pub fn arity(arguments: &[Json], count: usize) -> Result<()> {
    if arguments.len() == count {
        return Ok(());
    }
    Err(Error::Argument { message: format!("wrong number of arguments (given {}, expected {count})", arguments.len()) })
}

/// The Integer, Float, String or boolean argument at `index`, as the
/// signature types it; nil where it may be nil.
pub fn scalar_at<T: serde::de::DeserializeOwned>(arguments: &[Json], index: usize) -> Result<T> {
    let argument = arguments.get(index).cloned().unwrap_or(Json::Null);
    serde_json::from_value(argument).map_err(|_| deserialization(format!("argument {index} isn't a {}", std::any::type_name::<T>())))
}

fn deserialization(cause: String) -> Error {
    Error::Raised { class: "ActiveJob::DeserializationError", message: format!("Error while trying to deserialize arguments: {cause}") }
}

/// Active Job's job id, a random (version 4) UUID.
fn uuid() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS has randomness");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Sidekiq's jid: 12 random bytes in hex.
pub(crate) fn hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("the OS has randomness");
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}
