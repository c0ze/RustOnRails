# RustOnRails Record Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An Active Record layer in Rust (models, queries, validations, callbacks, persistence) complete enough that a hand-written port of `examples/blog`'s models passes Rust versions of all 13 of the blog's model tests.

**Architecture:** Synchronous, on the blocking `postgres` crate. A `Ctx` owns one connection and a record table; code refers to records through `Copy` `Handle<M>`s and indexes the `Ctx` to read or write them. Models are `model!`-declared structs with `Option` fields plus a static `Behavior` (enums, validations, callbacks in source order). This is plan 2 of the PoC. Plan 1 (Rutile, merged) built `rutile introspect`; plan 3 adds controllers, routing, JSON and associations and runs the blog's Rails integration tests against a hand-ported Rust server; plan 4 makes `rutile build` generate that port.

**Tech Stack:** Rust 1.95 (edition 2024), postgres 0.19 (`with-chrono-0_4`), chrono 0.4, bytes 1, regex 1. PostgreSQL 16 from the Rutile repo's throwaway cluster.

**Spec:** `docs/design.md` (sections "Synchronous code", "Memory model", "Records"). The Rails side of the port lives in `../Rutile/examples/blog`.

## Global Constraints

- Dependencies are exactly: `bytes = "1"`, `chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }`, `postgres = { version = "0.19", features = ["with-chrono-0_4"] }`, `regex = "1"`. No async runtime, no ORM crate, no proc-macro crate.
- Public names follow Rails, with Rust spelling where Ruby's can't be used: `create!` is `create_bang`, `valid?` is `is_valid`, `increment!` is `increment_bang`, `find_by!` is `find_by_bang`.
- Behavior matches Rails 8.1.4: validation messages (`can't be blank`, `is too long (maximum is 200 characters)`, `is invalid`, `has already been taken`, `is not included in the list`, `must exist`), `RecordNotFound` text (`Couldn't find Post with 'id'=0`), and callbacks running in definition order for both before and after callbacks.
- Tests need Postgres. Default URL `postgres://postgres@localhost:54329/rustonrails_test`; start the cluster with `cd ../Rutile && bundle exec rake pg:start`; override with `RUSTONRAILS_TEST_DATABASE_URL`. Every test runs in its own transaction that is never committed.
- Code files stay under 200 lines. No mocks outside `tests/`. Never create or overwrite a `.env` file.
- Every commit message ends with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run everything from the RustOnRails repo root with `cargo test`. `cargo clippy --all-targets` must stay free of errors.

## Review Focus

1. **Ruby nil and blank input.** Any attribute can be nil or a blank string, including integer columns fed `""` from params. The result must be a validation error or nil, never a panic or a cast error. Tests: Task 2 `test_integer_attributes_cast_like_active_model`, Task 4 `test_presence_rejects_nil_and_whitespace`.
2. **A failed save inside a transaction.** It must roll back only its own savepoint and leave the record new (id nil, `is_new_record` true), with earlier work in the same transaction intact. Test: Task 5 `test_failed_after_create_rolls_back_and_leaves_record_new`.
3. **Enum labels in queries.** `where_eq("status", "published")` must query the integer; an unknown label must match nothing rather than erroring. Test: Task 3 `test_unknown_enum_label_matches_nothing`.
4. **Values that look like SQL.** Values always travel as parameters and identifiers are quoted, so a quote character in a value is just data. Test: Task 3 `test_values_are_parameters_not_sql`.
5. **Timestamps across a round trip.** `now()` and saved timestamps must compare equal after `reload`, which means microsecond precision like a Rails `datetime(6)`. Test: Task 5 `test_timestamps_survive_reload`.

---

### Task 1: Values, errors, the connection and the test database

**Files:**
- Modify: `Cargo.toml`, `src/lib.rs`
- Create: `src/error.rs`, `src/value.rs`, `src/pg.rs`, `src/ctx.rs`
- Create: `tests/support/mod.rs`, `tests/support/schema.sql`
- Create: `tests/value_test.rs`, `tests/transaction_test.rs`

**Interfaces:**
- Produces:
  - `rustonrails::{Error, Result<T>}`; `Error` variants `RecordNotFound { model: &'static str, conditions: Option<String> }`, `RecordInvalid { model, messages: Vec<String> }`, `RecordNotSaved { model }`, `RecordNotDestroyed { model }`, `Abort`, `Nil { what: &'static str }`, `Cast { expected: &'static str, value: Value }`, `UnknownAttribute { model, name: String }`, `NotPersisted { model }`, `Db(postgres::Error)`
  - `rustonrails::{Value, Time, now, FromValue}`; `Value::{is_nil, is_blank, to_ruby_string}`; `From<bool|i32|i64|f64|&str|String|Time|Option<T>> for Value`; `FromValue::from_value(Value) -> Result<Option<Self>>` for `i64`, `f64`, `bool`, `String`, `Time`
  - `pg::{read(&Row, usize) -> Result<Value>, quote(&str) -> String}` (crate-private); `impl ToSql for Value`
  - `Ctx::{connect(&str), new(Client), rolled_back(Client), query(&str, &[Value]) -> Result<Vec<Row>>, execute(&str, &[Value]) -> Result<u64>, transaction(FnOnce(&mut Ctx) -> Result<bool>) -> Result<bool>}`
  - test helper `support::ctx() -> Ctx`

- [ ] **Step 1: Add the dependencies**

Replace `Cargo.toml`:

```toml
[package]
name = "rustonrails"
version = "0.1.0"
edition = "2024"

[dependencies]
bytes = "1"
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
postgres = { version = "0.19", features = ["with-chrono-0_4"] }
regex = "1"
```

- [ ] **Step 2: Dump the blog schema for the tests**

Run from the RustOnRails root (the cluster must be up: `cd ../Rutile && bundle exec rake example:db`):

```bash
mkdir -p tests/support
pg_dump -h localhost -p 54329 -U postgres --schema-only --no-owner --no-privileges --no-comments \
  -T schema_migrations -T ar_internal_metadata blog_test \
  | grep -v '^\\' | grep -v '^--' | cat -s > tests/support/schema.sql
grep -c 'CREATE TABLE' tests/support/schema.sql
```

Expected: `3`. The `grep -v '^\\'` drops `pg_dump`'s `\restrict` lines, which carry a random key and aren't SQL.

- [ ] **Step 3: Write the failing tests**

Create `tests/support/mod.rs`:

```rust
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Once;

use postgres::{Client, NoTls};
use rustonrails::Ctx;

const SCHEMA: &str = include_str!("schema.sql");

fn url() -> String {
    std::env::var("RUSTONRAILS_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:54329/rustonrails_test".into())
}

/// A `Ctx` inside a transaction that is never committed: dropping it rolls
/// everything back, so tests can run in parallel on one database.
pub fn ctx() -> Ctx {
    static SETUP: Once = Once::new();
    SETUP.call_once(prepare_database);
    let client = Client::connect(&url(), NoTls).expect("connect to the test database");
    Ctx::rolled_back(client).expect("begin the test transaction")
}

/// Creates the database on first use and reloads schema.sql whenever it
/// changes. An advisory lock keeps concurrent test runs from racing.
fn prepare_database() {
    let url = url();
    let (base, name) = url.rsplit_once('/').expect("database URL ends in /name");
    let mut admin = Client::connect(&format!("{base}/postgres"), NoTls).expect("connect to postgres");
    admin.batch_execute("SELECT pg_advisory_lock(7351)").unwrap();
    if admin.query("SELECT 1 FROM pg_database WHERE datname = $1", &[&name]).unwrap().is_empty() {
        admin.batch_execute(&format!("CREATE DATABASE \"{name}\"")).unwrap();
    }
    let mut db = Client::connect(&url, NoTls).unwrap();
    let mut hasher = DefaultHasher::new();
    SCHEMA.hash(&mut hasher);
    let digest = hasher.finish().to_string();
    let loaded: Option<String> =
        db.query_opt("SELECT digest FROM public.rustonrails_schema", &[]).ok().flatten().map(|row| row.get(0));
    if loaded.as_deref() != Some(digest.as_str()) {
        db.batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;").unwrap();
        db.batch_execute(SCHEMA).unwrap();
        db.batch_execute("CREATE TABLE public.rustonrails_schema (digest text NOT NULL)").unwrap();
        db.execute("INSERT INTO public.rustonrails_schema VALUES ($1)", &[&digest]).unwrap();
    }
    admin.batch_execute("SELECT pg_advisory_unlock(7351)").unwrap();
}
```

Create `tests/value_test.rs`:

```rust
use rustonrails::{FromValue, Time, Value, now};

#[test]
fn test_blank_follows_ruby() {
    assert!(Value::Nil.is_blank());
    assert!(Value::Bool(false).is_blank());
    assert!(Value::from("  \t").is_blank());
    assert!(!Value::from("x").is_blank());
    assert!(!Value::Int(0).is_blank());
}

#[test]
fn test_options_convert_to_nil() {
    assert_eq!(Value::Nil, Value::from(None::<String>));
    assert_eq!(Value::Int(3), Value::from(Some(3_i64)));
}

#[test]
fn test_integer_casting_matches_active_model() {
    assert_eq!(Some(42), i64::from_value(Value::from("42")).unwrap());
    assert_eq!(Some(12), i64::from_value(Value::from(" 12abc")).unwrap());
    assert_eq!(None, i64::from_value(Value::from("abc")).unwrap());
    assert_eq!(None, i64::from_value(Value::from("")).unwrap());
    assert_eq!(Some(1), i64::from_value(Value::Bool(true)).unwrap());
}

#[test]
fn test_string_and_boolean_casting() {
    assert_eq!(Some("7".to_string()), String::from_value(Value::Int(7)).unwrap());
    assert_eq!(Some("t".to_string()), String::from_value(Value::Bool(true)).unwrap());
    assert_eq!(Some(false), bool::from_value(Value::from("0")).unwrap());
    assert_eq!(Some(true), bool::from_value(Value::from("yes")).unwrap());
    assert_eq!(None, bool::from_value(Value::from("")).unwrap());
}

#[test]
fn test_now_has_microsecond_precision() {
    let time: Time = now();
    assert_eq!(0, time.and_utc().timestamp_subsec_nanos() % 1_000);
}
```

Create `tests/transaction_test.rs`:

```rust
mod support;

use rustonrails::{Error, Value};

fn count(ctx: &mut rustonrails::Ctx) -> i64 {
    ctx.query("SELECT COUNT(*) FROM users", &[]).unwrap()[0].get(0)
}

fn insert_user(ctx: &mut rustonrails::Ctx, email: &str) {
    ctx.execute(
        "INSERT INTO users (name, email, created_at, updated_at) VALUES ($1, $2, now(), now())",
        &[Value::from("Test"), Value::from(email)],
    )
    .unwrap();
}

#[test]
fn test_each_ctx_sees_only_its_own_rows() {
    let mut first = support::ctx();
    let mut second = support::ctx();
    insert_user(&mut first, "first@example.com");
    assert_eq!(1, count(&mut first));
    assert_eq!(0, count(&mut second));
}

#[test]
fn test_transaction_returning_false_rolls_back() {
    let mut ctx = support::ctx();
    let kept = ctx.transaction(|ctx| { insert_user(ctx, "gone@example.com"); Ok(false) }).unwrap();
    assert!(!kept);
    assert_eq!(0, count(&mut ctx));
}

#[test]
fn test_nested_failure_keeps_outer_work() {
    let mut ctx = support::ctx();
    let outer = ctx.transaction(|ctx| {
        insert_user(ctx, "kept@example.com");
        let inner = ctx.transaction(|ctx| { insert_user(ctx, "dropped@example.com"); Err(Error::Abort) });
        assert!(matches!(inner, Err(Error::Abort)));
        Ok(true)
    });
    assert!(outer.unwrap());
    assert_eq!(1, count(&mut ctx));
}

#[test]
fn test_database_errors_surface() {
    let mut ctx = support::ctx();
    let result = ctx.execute("INSERT INTO users (name) VALUES ($1)", &[Value::from("no email")]);
    assert!(matches!(result, Err(Error::Db(_))));
}
```

- [ ] **Step 4: Run to see them fail**

Run: `cargo test 2>&1 | tail -5`
Expected: compile errors, `unresolved imports rustonrails::FromValue` / `rustonrails::Ctx` and similar.

- [ ] **Step 5: Implement**

Create `src/error.rs`:

```rust
use std::fmt;

use crate::Value;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What the record layer raises where Rails would raise an exception.
#[derive(Debug)]
pub enum Error {
    /// `ActiveRecord::RecordNotFound`
    RecordNotFound { model: &'static str, conditions: Option<String> },
    /// `ActiveRecord::RecordInvalid`, from the bang methods.
    RecordInvalid { model: &'static str, messages: Vec<String> },
    /// `ActiveRecord::RecordNotSaved`: a callback stopped `save!`.
    RecordNotSaved { model: &'static str },
    /// `ActiveRecord::RecordNotDestroyed`: a callback stopped `destroy!`.
    RecordNotDestroyed { model: &'static str },
    /// A before callback stopped the chain, like `throw :abort`.
    Abort,
    /// A method called on nil, Ruby's `NoMethodError` for `nil`.
    Nil { what: &'static str },
    /// A value an attribute's type can't hold.
    Cast { expected: &'static str, value: Value },
    UnknownAttribute { model: &'static str, name: String },
    /// Writing a record that was never saved, like `increment!` on a new one.
    NotPersisted { model: &'static str },
    /// Anything the database reported.
    Db(postgres::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::RecordNotFound { model, conditions: Some(c) } => write!(f, "Couldn't find {model} with {c}"),
            Error::RecordNotFound { model, conditions: None } => write!(f, "Couldn't find {model}"),
            Error::RecordInvalid { messages, .. } => write!(f, "Validation failed: {}", messages.join(", ")),
            Error::RecordNotSaved { .. } => write!(f, "Failed to save the record"),
            Error::RecordNotDestroyed { model } => write!(f, "Failed to destroy {model}"),
            Error::Abort => write!(f, "callback chain aborted"),
            Error::Nil { what } => write!(f, "undefined method '{what}' for nil"),
            Error::Cast { expected, value } => write!(f, "can't cast {value:?} to {expected}"),
            Error::UnknownAttribute { model, name } => write!(f, "unknown attribute '{name}' for {model}"),
            Error::NotPersisted { model } => write!(f, "cannot update a new {model}"),
            Error::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<postgres::Error> for Error {
    fn from(e: postgres::Error) -> Self {
        Error::Db(e)
    }
}
```

Create `src/value.rs`:

```rust
use chrono::{NaiveDateTime, SubsecRound, Utc};

use crate::{Error, Result};

/// Times are UTC without a zone, the way Rails stores `datetime` columns.
pub type Time = NaiveDateTime;

/// `Time.current`, rounded to microseconds like a Rails `datetime(6)`
/// attribute, so it compares equal after a trip through the database.
pub fn now() -> Time {
    Utc::now().naive_utc().trunc_subsecs(6)
}

/// A Ruby value as the record layer sees it: attribute reads and writes by
/// name, query parameters, and the fallback for code Rutile couldn't type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Time(Time),
}

impl Value {
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Ruby's `blank?`: nil, false, and strings that are empty or whitespace.
    pub fn is_blank(&self) -> bool {
        match self {
            Value::Nil | Value::Bool(false) => true,
            Value::Str(s) => s.trim().is_empty(),
            _ => false,
        }
    }

    /// `to_s` as validators use it; nil becomes "".
    pub fn to_ruby_string(&self) -> String {
        match self {
            Value::Nil => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Str(s) => s.clone(),
            Value::Time(t) => t.to_string(),
        }
    }
}

impl From<bool> for Value { fn from(v: bool) -> Self { Value::Bool(v) } }
impl From<i32> for Value { fn from(v: i32) -> Self { Value::Int(v.into()) } }
impl From<i64> for Value { fn from(v: i64) -> Self { Value::Int(v) } }
impl From<f64> for Value { fn from(v: f64) -> Self { Value::Float(v) } }
impl From<&str> for Value { fn from(v: &str) -> Self { Value::Str(v.to_string()) } }
impl From<String> for Value { fn from(v: String) -> Self { Value::Str(v) } }
impl From<Time> for Value { fn from(v: Time) -> Self { Value::Time(v) } }

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Nil, Into::into)
    }
}

/// Casts an assigned value to an attribute's type the way Active Model
/// types do: "42" becomes 42 for an integer column, "" becomes nil.
pub trait FromValue: Sized {
    fn from_value(value: Value) -> Result<Option<Self>>;
}

impl FromValue for i64 {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Int(i) => Ok(Some(i)),
            Value::Float(f) => Ok(Some(f as i64)),
            Value::Bool(b) => Ok(Some(b.into())),
            Value::Str(s) => Ok(leading_integer(&s)),
            other => Err(Error::Cast { expected: "integer", value: other }),
        }
    }
}

/// `"12abc".to_i` is 12, but Active Model casts a string with no leading
/// digits to nil rather than 0.
fn leading_integer(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let sign_len = usize::from(s.starts_with(['+', '-']));
    let digits = s[sign_len..].chars().take_while(char::is_ascii_digit).count();
    if digits == 0 { None } else { s[..sign_len + digits].parse().ok() }
}

impl FromValue for f64 {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Float(f) => Ok(Some(f)),
            Value::Int(i) => Ok(Some(i as f64)),
            Value::Str(s) if s.trim().is_empty() => Ok(None),
            Value::Str(s) => Ok(s.trim().parse().ok()),
            other => Err(Error::Cast { expected: "float", value: other }),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Bool(b) => Ok(Some(b)),
            Value::Int(i) => Ok(Some(i != 0)),
            Value::Str(s) if s.is_empty() => Ok(None),
            Value::Str(s) => Ok(Some(!matches!(s.as_str(), "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF"))),
            other => Err(Error::Cast { expected: "boolean", value: other }),
        }
    }
}

impl FromValue for String {
    fn from_value(value: Value) -> Result<Option<Self>> {
        Ok(match value {
            Value::Nil => None,
            Value::Bool(b) => Some(if b { "t" } else { "f" }.to_string()),
            other => Some(other.to_ruby_string()),
        })
    }
}

impl FromValue for Time {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Time(t) => Ok(Some(t)),
            Value::Str(s) => Ok(["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
                .iter()
                .find_map(|format| NaiveDateTime::parse_from_str(s.trim().trim_end_matches('Z'), format).ok())),
            other => Err(Error::Cast { expected: "datetime", value: other }),
        }
    }
}
```

Create `src/pg.rs`:

```rust
use std::error::Error as StdError;

use bytes::BytesMut;
use postgres::Row;
use postgres::types::{IsNull, ToSql, Type, to_sql_checked};

use crate::{Result, Time, Value};

/// Double-quotes an identifier. Identifiers come from generated code, never
/// from user input; values always go as parameters.
pub(crate) fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// Reads one column of a row as a `Value`, going by its Postgres type.
pub(crate) fn read(row: &Row, index: usize) -> Result<Value> {
    let ty = row.columns()[index].type_().clone();
    let value = if ty == Type::INT8 {
        row.try_get::<_, Option<i64>>(index)?.map(Value::Int)
    } else if ty == Type::INT4 {
        row.try_get::<_, Option<i32>>(index)?.map(Value::from)
    } else if ty == Type::INT2 {
        row.try_get::<_, Option<i16>>(index)?.map(|v| Value::Int(v.into()))
    } else if ty == Type::BOOL {
        row.try_get::<_, Option<bool>>(index)?.map(Value::Bool)
    } else if ty == Type::FLOAT8 {
        row.try_get::<_, Option<f64>>(index)?.map(Value::Float)
    } else if ty == Type::TIMESTAMP {
        row.try_get::<_, Option<Time>>(index)?.map(Value::Time)
    } else {
        row.try_get::<_, Option<String>>(index)?.map(Value::Str)
    };
    Ok(value.unwrap_or(Value::Nil))
}

impl ToSql for Value {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> std::result::Result<IsNull, Box<dyn StdError + Sync + Send>> {
        match self {
            Value::Nil => Ok(IsNull::Yes),
            Value::Bool(b) => b.to_sql(ty, out),
            Value::Int(i) if *ty == Type::INT4 => i32::try_from(*i)?.to_sql(ty, out),
            Value::Int(i) if *ty == Type::INT2 => i16::try_from(*i)?.to_sql(ty, out),
            Value::Int(i) if *ty == Type::FLOAT8 => (*i as f64).to_sql(ty, out),
            Value::Int(i) => i.to_sql(ty, out),
            Value::Float(f) => f.to_sql(ty, out),
            Value::Str(s) => s.to_sql(ty, out),
            Value::Time(t) => t.to_sql(ty, out),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}
```

Create `src/ctx.rs`:

```rust
use postgres::types::ToSql;
use postgres::{Client, NoTls, Row};

use crate::{Result, Value};

/// One unit of work: a database connection plus, from Task 2 on, every
/// record it loaded or built. Rails' equivalent is the objects a request
/// creates; they all go away when the `Ctx` is dropped.
pub struct Ctx {
    client: Client,
    depth: u32,
}

impl Ctx {
    pub fn connect(url: &str) -> Result<Self> {
        Ok(Self::new(Client::connect(url, NoTls)?))
    }

    pub fn new(client: Client) -> Self {
        Self { client, depth: 0 }
    }

    /// Opens a transaction that is never committed; dropping the `Ctx`
    /// rolls it back. For tests, like Rails' transactional fixtures.
    pub fn rolled_back(mut client: Client) -> Result<Self> {
        client.batch_execute("BEGIN")?;
        Ok(Self { client, depth: 1 })
    }

    pub fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        Ok(self.client.query(sql, &refs(params))?)
    }

    pub fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        Ok(self.client.execute(sql, &refs(params))?)
    }

    /// `transaction do ... end`, keeping the block's outcome the way `save`
    /// does: `Ok(false)` or an error rolls back. Nested calls use savepoints.
    pub fn transaction(&mut self, block: impl FnOnce(&mut Ctx) -> Result<bool>) -> Result<bool> {
        let name = format!("rustonrails_{}", self.depth);
        let (begin, commit, rollback) = if self.depth == 0 {
            ("BEGIN".to_string(), "COMMIT".to_string(), "ROLLBACK".to_string())
        } else {
            (format!("SAVEPOINT {name}"), format!("RELEASE SAVEPOINT {name}"), format!("ROLLBACK TO SAVEPOINT {name}"))
        };
        self.client.batch_execute(&begin)?;
        self.depth += 1;
        let outcome = block(self);
        self.depth -= 1;
        match outcome {
            Ok(true) => {
                self.client.batch_execute(&commit)?;
                Ok(true)
            }
            Ok(false) => {
                self.client.batch_execute(&rollback)?;
                Ok(false)
            }
            Err(error) => {
                // The original error says more than a failed rollback would.
                self.client.batch_execute(&rollback).ok();
                Err(error)
            }
        }
    }
}

fn refs(params: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
}
```

Replace `src/lib.rs`:

```rust
//! RustOnRails: the Rails API surface in Rust.
//!
//! This is the runtime that code generated by Rutile links against. The
//! record layer mirrors Active Record: models are plain structs, a `Ctx`
//! owns the database connection and every record one unit of work touches,
//! and code refers to records through `Handle`s. See `docs/design.md`.

mod ctx;
mod error;
mod pg;
mod value;

pub use ctx::Ctx;
pub use error::{Error, Result};
pub use value::{FromValue, Time, Value, now};
```

- [ ] **Step 6: Run the tests**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|error'`
Expected: every `test result:` line reads `ok`; `value_test` 5 passed, `transaction_test` 4 passed.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src tests
git commit -m "Record layer foundation: values, errors, connection, test database" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Records, behavior declarations and handles

**Files:**
- Create: `src/model.rs`, `src/behavior.rs`, `src/validation.rs`
- Modify: `src/ctx.rs`, `src/lib.rs`
- Create: `tests/record_test.rs`

**Interfaces:**
- Consumes: Task 1's `Value`, `FromValue`, `Error`, `Ctx`.
- Produces:
  - `trait Record: Clone + Default + PartialEq + Send + 'static` with `NAME`, `TABLE`, `COLUMNS`, `new_record()`, `get(&str) -> Value`, `set(&str, Value) -> Result<()>`, `id() -> Option<i64>`
  - `trait Model: Record` with `behavior() -> &'static Behavior<Self>` (finder defaults arrive in Task 3)
  - `model! { pub struct Name in "table" { field: Type [= default], ... } }`
  - `Behavior<M>` builder: `new`, `belongs_to::<T>(name, foreign_key)`, `enumeration(attribute, &[(label, int)], validate)`, `validates(attribute, Check)`, `validate(Hook)`, `callback(Event, Hook)`, `before_validation`, `after_validation`, `before_save`, `after_save`, `before_create`, `after_create`, `before_update`, `after_update`, `before_destroy`, `after_destroy`, `when(Cond)`, `unless(Cond)`; crate-private `to_database(attribute, Value)`, `from_database(attribute, Value)`
  - `Check::{Presence, Length { minimum, maximum }, Format(Regex), Uniqueness, Inclusion(Vec<Value>), Required { foreign_key, table }}`; `Event` enum; `Hook<M> = fn(&mut Ctx, Handle<M>) -> Result<()>`; `Cond<M> = fn(&Ctx, Handle<M>) -> bool`
  - `Errors::{add, on, is_empty, clear, attributes, full_messages}`
  - `Handle<M>` (`Copy`, `Eq`, `Debug`); `Ctx::{build, errors, errors_mut, is_new_record, is_persisted, is_destroyed, changed, attribute_changed}`; `Index`/`IndexMut<Handle<M>> for Ctx`; crate-private `Ctx::{adopt, slot, slot_mut}` and `Slot { record, saved: Option<M>, errors, destroyed }`

- [ ] **Step 1: Write the failing test**

Create `tests/record_test.rs`:

```rust
mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Model, Record, Time, Value, model};

model! {
    pub struct Note in "posts" {
        id: i64,
        title: String,
        status: String = "draft",
        comments_count: i64 = 0,
        published_at: Time,
    }
}

impl Model for Note {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Note>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

#[test]
fn test_model_macro_describes_the_table() {
    assert_eq!("Note", Note::NAME);
    assert_eq!("posts", Note::TABLE);
    assert_eq!(&["id", "title", "status", "comments_count", "published_at"], Note::COLUMNS);
}

#[test]
fn test_new_record_takes_column_defaults() {
    let note = Note::new_record();
    assert_eq!(Some("draft".to_string()), note.status);
    assert_eq!(Some(0), note.comments_count);
    assert_eq!(None, note.title);
}

#[test]
fn test_attributes_by_name() {
    let mut note = Note::new_record();
    note.set("title", Value::from("Hi")).unwrap();
    assert_eq!(Value::from("Hi"), note.get("title"));
    assert_eq!(Value::Nil, note.get("published_at"));
    assert!(note.set("nope", Value::Nil).is_err());
}

#[test]
fn test_integer_attributes_cast_like_active_model() {
    let mut note = Note::new_record();
    note.set("comments_count", Value::from("")).unwrap();
    assert_eq!(None, note.comments_count);
    note.set("comments_count", Value::from("3")).unwrap();
    assert_eq!(Some(3), note.comments_count);
}

#[test]
fn test_handles_alias_like_ruby_variables() {
    let mut ctx = support::ctx();
    let a = ctx.build(Note::new_record());
    let b = a;
    ctx[b].title = Some("x".into());
    assert_eq!(Some("x"), ctx[a].title.as_deref());
    assert!(ctx.is_new_record(a));
    assert!(!ctx.is_persisted(a));
}

#[test]
fn test_changed_compares_with_defaults_for_new_records() {
    let mut ctx = support::ctx();
    let note = ctx.build(Note::new_record());
    assert!(ctx.changed(note).is_empty());
    ctx[note].title = Some("x".into());
    assert_eq!(vec!["title"], ctx.changed(note));
    assert!(ctx.attribute_changed(note, "title"));
}

#[test]
fn test_errors_keep_order_and_humanize() {
    let mut ctx = support::ctx();
    let note = ctx.build(Note::new_record());
    ctx.errors_mut(note).add("title", "can't be blank");
    ctx.errors_mut(note).add("user_id", "must exist");
    ctx.errors_mut(note).add("title", "is too short (minimum is 3 characters)");
    let errors = ctx.errors(note);
    assert_eq!(vec!["can't be blank", "is too short (minimum is 3 characters)"], errors.on("title"));
    assert_eq!(vec!["title", "user_id"], errors.attributes());
    assert_eq!("User must exist", errors.full_messages()[1]);
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --test record_test 2>&1 | tail -3`
Expected: compile error, `unresolved imports rustonrails::Behavior, rustonrails::Model, rustonrails::Record`.

- [ ] **Step 3: Implement the model traits and macro**

Create `src/model.rs`:

```rust
use crate::{Behavior, Result, Value};

/// Column-level plumbing for a model struct. `model!` implements it.
pub trait Record: Clone + Default + PartialEq + Send + 'static {
    const NAME: &'static str;
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];

    /// An unsaved record holding the columns' database defaults, like `Post.new`.
    fn new_record() -> Self;
    fn get(&self, column: &str) -> Value;
    fn set(&mut self, column: &str, value: Value) -> Result<()>;

    fn id(&self) -> Option<i64> {
        match self.get("id") {
            Value::Int(id) => Some(id),
            _ => None,
        }
    }
}

/// A record plus its class-level declarations. Class methods such as
/// `Post.find` are default methods here (Task 3).
pub trait Model: Record {
    fn behavior() -> &'static Behavior<Self>;
}

/// Declares a model struct: one `Option` field per column, since any Ruby
/// attribute can be nil until the database says otherwise, plus `Record`.
/// `= default` gives the column's database default.
#[macro_export]
macro_rules! model {
    ($(#[$meta:meta])* $vis:vis struct $name:ident in $table:literal {
        $($field:ident : $ty:ty $(= $default:expr)?),* $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq)]
        $vis struct $name { $(pub $field: Option<$ty>),* }

        impl $crate::Record for $name {
            const NAME: &'static str = stringify!($name);
            const TABLE: &'static str = $table;
            const COLUMNS: &'static [&'static str] = &[$(stringify!($field)),*];

            fn new_record() -> Self {
                Self { $($field: $crate::__model_default!($($default)?)),* }
            }

            fn get(&self, column: &str) -> $crate::Value {
                match column {
                    $(stringify!($field) => $crate::Value::from(self.$field.clone()),)*
                    _ => $crate::Value::Nil,
                }
            }

            fn set(&mut self, column: &str, value: $crate::Value) -> $crate::Result<()> {
                match column {
                    $(stringify!($field) => {
                        self.$field = <$ty as $crate::FromValue>::from_value(value)?;
                        Ok(())
                    })*
                    _ => Err($crate::Error::UnknownAttribute { model: stringify!($name), name: column.to_string() }),
                }
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __model_default {
    () => { None };
    ($default:expr) => { Some(($default).into()) };
}
```

- [ ] **Step 4: Implement behavior declarations and errors**

Create `src/behavior.rs`:

```rust
use regex::Regex;

use crate::{Ctx, Handle, Record, Result, Value};

/// A callback or a custom validation, like `before_save :stamp_published_at`.
pub type Hook<M> = fn(&mut Ctx, Handle<M>) -> Result<()>;
/// An `if:` or `unless:` condition.
pub type Cond<M> = fn(&Ctx, Handle<M>) -> bool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    BeforeValidation,
    AfterValidation,
    BeforeSave,
    AfterSave,
    BeforeCreate,
    AfterCreate,
    BeforeUpdate,
    AfterUpdate,
    BeforeDestroy,
    AfterDestroy,
}

/// The built-in validators, one per `validates` option.
pub enum Check {
    Presence,
    Length { minimum: Option<usize>, maximum: Option<usize> },
    Format(Regex),
    Uniqueness,
    Inclusion(Vec<Value>),
    /// What `belongs_to` adds unless `optional: true`: the row must exist.
    Required { foreign_key: &'static str, table: &'static str },
}

pub(crate) enum Validation<M> {
    Check { attribute: &'static str, check: Check },
    Custom(Hook<M>),
}

/// An entry with its `if:` / `unless:` conditions.
pub(crate) struct Guarded<T, M> {
    pub item: T,
    when: Vec<Cond<M>>,
    unless: Vec<Cond<M>>,
}

impl<T, M> Guarded<T, M> {
    pub fn applies(&self, ctx: &Ctx, record: Handle<M>) -> bool {
        self.when.iter().all(|c| c(ctx, record)) && !self.unless.iter().any(|c| c(ctx, record))
    }
}

pub(crate) struct EnumDef {
    attribute: &'static str,
    mapping: Vec<(&'static str, i64)>,
}

enum Last {
    Validation,
    Callback,
}

/// A model's class-level declarations, in source order, because order is
/// behavior: validations and callbacks run in the order they were declared.
pub struct Behavior<M> {
    enums: Vec<EnumDef>,
    pub(crate) validations: Vec<Guarded<Validation<M>, M>>,
    pub(crate) callbacks: Vec<Guarded<(Event, Hook<M>), M>>,
    last: Option<Last>,
}

impl<M> Default for Behavior<M> {
    fn default() -> Self {
        Self { enums: Vec::new(), validations: Vec::new(), callbacks: Vec::new(), last: None }
    }
}

impl<M> Behavior<M> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `belongs_to :user`: adds the "must exist" check Rails adds.
    pub fn belongs_to<T: Record>(self, name: &'static str, foreign_key: &'static str) -> Self {
        self.validates(name, Check::Required { foreign_key, table: T::TABLE })
    }

    /// `enum :status, { draft: 0 }`; `validate` is Rails' `validate: true`.
    pub fn enumeration(mut self, attribute: &'static str, mapping: &[(&'static str, i64)], validate: bool) -> Self {
        self.enums.push(EnumDef { attribute, mapping: mapping.to_vec() });
        if !validate {
            return self;
        }
        let labels = mapping.iter().map(|(label, _)| Value::from(*label)).collect();
        self.validates(attribute, Check::Inclusion(labels))
    }

    pub fn validates(mut self, attribute: &'static str, check: Check) -> Self {
        self.validations.push(Guarded { item: Validation::Check { attribute, check }, when: vec![], unless: vec![] });
        self.last = Some(Last::Validation);
        self
    }

    /// `validate :method`
    pub fn validate(mut self, hook: Hook<M>) -> Self {
        self.validations.push(Guarded { item: Validation::Custom(hook), when: vec![], unless: vec![] });
        self.last = Some(Last::Validation);
        self
    }

    pub fn callback(mut self, event: Event, hook: Hook<M>) -> Self {
        self.callbacks.push(Guarded { item: (event, hook), when: vec![], unless: vec![] });
        self.last = Some(Last::Callback);
        self
    }

    pub fn before_validation(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeValidation, hook) }
    pub fn after_validation(self, hook: Hook<M>) -> Self { self.callback(Event::AfterValidation, hook) }
    pub fn before_save(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeSave, hook) }
    pub fn after_save(self, hook: Hook<M>) -> Self { self.callback(Event::AfterSave, hook) }
    pub fn before_create(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeCreate, hook) }
    pub fn after_create(self, hook: Hook<M>) -> Self { self.callback(Event::AfterCreate, hook) }
    pub fn before_update(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeUpdate, hook) }
    pub fn after_update(self, hook: Hook<M>) -> Self { self.callback(Event::AfterUpdate, hook) }
    pub fn before_destroy(self, hook: Hook<M>) -> Self { self.callback(Event::BeforeDestroy, hook) }
    pub fn after_destroy(self, hook: Hook<M>) -> Self { self.callback(Event::AfterDestroy, hook) }

    /// `if:` on the validation or callback declared just before.
    pub fn when(mut self, cond: Cond<M>) -> Self {
        match self.last {
            Some(Last::Validation) => self.validations.last_mut().expect("declared").when.push(cond),
            Some(Last::Callback) => self.callbacks.last_mut().expect("declared").when.push(cond),
            None => panic!("`when` needs a validation or callback before it"),
        }
        self
    }

    /// `unless:` on the validation or callback declared just before.
    pub fn unless(mut self, cond: Cond<M>) -> Self {
        match self.last {
            Some(Last::Validation) => self.validations.last_mut().expect("declared").unless.push(cond),
            Some(Last::Callback) => self.callbacks.last_mut().expect("declared").unless.push(cond),
            None => panic!("`unless` needs a validation or callback before it"),
        }
        self
    }

    /// Enum labels become their integers; an unknown label becomes nil, as
    /// Rails' enum type serializes it.
    pub(crate) fn to_database(&self, attribute: &str, value: Value) -> Value {
        match self.enums.iter().find(|e| e.attribute == attribute) {
            Some(def) => match &value {
                Value::Str(label) => def.mapping.iter().find(|(l, _)| l == label).map_or(Value::Nil, |(_, i)| Value::Int(*i)),
                _ => value,
            },
            None => value,
        }
    }

    /// Enum integers become their labels; an unknown integer becomes nil.
    pub(crate) fn from_database(&self, attribute: &str, value: Value) -> Value {
        match self.enums.iter().find(|e| e.attribute == attribute) {
            Some(def) => match value {
                Value::Int(i) => def.mapping.iter().find(|(_, n)| *n == i).map_or(Value::Nil, |(l, _)| Value::from(*l)),
                other => other,
            },
            None => value,
        }
    }
}
```

Create `src/validation.rs` (Task 4 adds the validator logic below `Errors`):

```rust
/// `record.errors`: messages per attribute, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Errors {
    entries: Vec<(String, String)>,
}

impl Errors {
    pub fn add(&mut self, attribute: &str, message: impl Into<String>) {
        self.entries.push((attribute.to_string(), message.into()));
    }

    /// `errors[:title]`
    pub fn on(&self, attribute: &str) -> Vec<&str> {
        self.entries.iter().filter(|(a, _)| a == attribute).map(|(_, m)| m.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Attributes with errors, first appearance first, as in `errors.as_json`.
    pub fn attributes(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for (attribute, _) in &self.entries {
            if !seen.contains(&attribute.as_str()) {
                seen.push(attribute);
            }
        }
        seen
    }

    /// "Title can't be blank", as in `errors.full_messages`.
    pub fn full_messages(&self) -> Vec<String> {
        self.entries.iter().map(|(a, m)| format!("{} {m}", humanize(a))).collect()
    }
}

/// `"user_id".humanize` is "User"; `"comments_count"` is "Comments count".
fn humanize(attribute: &str) -> String {
    let words = attribute.strip_suffix("_id").unwrap_or(attribute).replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}
```

- [ ] **Step 5: Add the record table to `Ctx`**

In `src/ctx.rs`, replace the `use` lines and the struct with:

```rust
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use postgres::types::ToSql;
use postgres::{Client, NoTls, Row};

use crate::{Errors, Model, Result, Value};

/// Refers to a record loaded into a `Ctx`, the way a Ruby variable refers
/// to an object: copying a handle doesn't copy the record.
pub struct Handle<M> {
    index: u32,
    marker: PhantomData<fn() -> M>,
}

impl<M> Clone for Handle<M> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M> Copy for Handle<M> {}
impl<M> PartialEq for Handle<M> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}
impl<M> Eq for Handle<M> {}
impl<M> std::fmt::Debug for Handle<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Handle({})", self.index)
    }
}

/// What the record table keeps for each record.
pub(crate) struct Slot<M> {
    pub record: M,
    /// The record as last loaded or saved; `None` until it's in the database.
    pub saved: Option<M>,
    pub errors: Errors,
    pub destroyed: bool,
}

/// One unit of work: a database connection plus every record it loaded or
/// built. Rails' equivalent is the objects a request creates; they all go
/// away when the `Ctx` is dropped. There's no identity map: like Rails,
/// every load makes a new record.
pub struct Ctx {
    client: Client,
    depth: u32,
    tables: HashMap<TypeId, Box<dyn Any + Send>>,
}
```

Change the two constructors so they initialize `tables`: in `new`, `Self { client, depth: 0, tables: HashMap::new() }`; in `rolled_back`, `Ok(Self { client, depth: 1, tables: HashMap::new() })`.

Add a second `impl Ctx` block below the first:

```rust
impl Ctx {
    /// `Post.new(...)`: adds an unsaved record.
    pub fn build<M: Model>(&mut self, record: M) -> Handle<M> {
        self.push(Slot { record, saved: None, errors: Errors::default(), destroyed: false })
    }

    /// Adds a record just read from the database.
    pub(crate) fn adopt<M: Model>(&mut self, record: M) -> Handle<M> {
        let saved = Some(record.clone());
        self.push(Slot { record, saved, errors: Errors::default(), destroyed: false })
    }

    fn push<M: Model>(&mut self, slot: Slot<M>) -> Handle<M> {
        let slots = self.slots_mut::<M>();
        slots.push(slot);
        Handle { index: (slots.len() - 1) as u32, marker: PhantomData }
    }

    fn slots_mut<M: Model>(&mut self) -> &mut Vec<Slot<M>> {
        self.tables
            .entry(TypeId::of::<M>())
            .or_insert_with(|| Box::new(Vec::<Slot<M>>::new()))
            .downcast_mut()
            .expect("record table holds one model type")
    }

    pub(crate) fn slot<M: Model>(&self, record: Handle<M>) -> &Slot<M> {
        let slots: &Vec<Slot<M>> = self
            .tables
            .get(&TypeId::of::<M>())
            .and_then(|table| table.downcast_ref())
            .expect("handle belongs to this Ctx");
        &slots[record.index as usize]
    }

    pub(crate) fn slot_mut<M: Model>(&mut self, record: Handle<M>) -> &mut Slot<M> {
        &mut self.slots_mut::<M>()[record.index as usize]
    }

    pub fn errors<M: Model>(&self, record: Handle<M>) -> &Errors {
        &self.slot(record).errors
    }

    pub fn errors_mut<M: Model>(&mut self, record: Handle<M>) -> &mut Errors {
        &mut self.slot_mut(record).errors
    }

    pub fn is_new_record<M: Model>(&self, record: Handle<M>) -> bool {
        self.slot(record).saved.is_none()
    }

    pub fn is_persisted<M: Model>(&self, record: Handle<M>) -> bool {
        let slot = self.slot(record);
        slot.saved.is_some() && !slot.destroyed
    }

    pub fn is_destroyed<M: Model>(&self, record: Handle<M>) -> bool {
        self.slot(record).destroyed
    }

    /// `changed`: columns that differ from the last saved state, or from
    /// the column defaults for a record that was never saved.
    pub fn changed<M: Model>(&self, record: Handle<M>) -> Vec<&'static str> {
        let slot = self.slot(record);
        let base = slot.saved.clone().unwrap_or_else(M::new_record);
        M::COLUMNS.iter().copied().filter(|c| base.get(c) != slot.record.get(c)).collect()
    }

    /// `attribute_changed?(column)`
    pub fn attribute_changed<M: Model>(&self, record: Handle<M>, column: &str) -> bool {
        self.changed(record).contains(&column)
    }
}

impl<M: Model> Index<Handle<M>> for Ctx {
    type Output = M;

    fn index(&self, record: Handle<M>) -> &M {
        &self.slot(record).record
    }
}

impl<M: Model> IndexMut<Handle<M>> for Ctx {
    fn index_mut(&mut self, record: Handle<M>) -> &mut M {
        &mut self.slot_mut(record).record
    }
}
```

Replace `src/lib.rs`'s module list and exports:

```rust
mod behavior;
mod ctx;
mod error;
mod model;
mod pg;
mod validation;
mod value;

pub use behavior::{Behavior, Check, Cond, Event, Hook};
pub use ctx::{Ctx, Handle};
pub use error::{Error, Result};
pub use model::{Model, Record};
pub use validation::Errors;
pub use value::{FromValue, Time, Value, now};
```

(Keep the crate doc comment at the top.)

- [ ] **Step 6: Run the tests**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|^error'`
Expected: all `ok`; `record_test` 7 passed.

- [ ] **Step 7: Commit**

```bash
git add src tests
git commit -m "Records: model! structs, behavior declarations, handles and the record table" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Queries and the blog models

**Files:**
- Create: `src/relation.rs`
- Modify: `src/model.rs`, `src/lib.rs`
- Create: `tests/blog/mod.rs`, `tests/relation_test.rs`

**Interfaces:**
- Consumes: Task 2's `Record`, `Model`, `Behavior`, `Ctx::adopt`.
- Produces:
  - `Relation<M>` (`Clone`): `new`, `where_eq(column, impl Into<Value>)`, `where_not(column, value)`, `where_gte(column, value)`, `order_asc(column)`, `order_desc(column)`, `limit(i64)`, `load(&mut Ctx) -> Result<Vec<Handle<M>>>`, `first(&mut Ctx) -> Result<Option<Handle<M>>>`, `count(&mut Ctx) -> Result<i64>`, `exists(&mut Ctx) -> Result<bool>`, `find(&mut Ctx, i64) -> Result<Handle<M>>`, `find_by(&mut Ctx, column, value) -> Result<Option<Handle<M>>>`, `find_by_bang(...) -> Result<Handle<M>>`, `to_sql() -> (String, Vec<Value>)`; crate-private `fetch(&mut Ctx) -> Result<Vec<M>>` and `from_row::<M>(&Row) -> Result<M>`
  - `Model` default methods: `all()`, `find(ctx, id)`, `find_by(ctx, column, value)`, `find_by_bang(ctx, column, value)`
  - `tests/blog/mod.rs`: `User`, `Post`, `Comment` structs; `Post::{is_published, is_draft}`; traits `PostScopes { recent, visible }` and `ApplicationRecordScopes { created_since }`; `Model` impls (Task 7 completes the behaviors)

- [ ] **Step 1: Write the blog models so far**

Create `tests/blog/mod.rs`:

```rust
#![allow(dead_code)]
//! examples/blog's models (../Rutile/examples/blog/app/models), written the
//! way `rutile build` should generate them. Comments point at the Ruby.

use std::sync::LazyLock;

use rustonrails::{Behavior, Model, Relation, Time, model};

// application_record.rb:4  scope :created_since, ->(time) { where(created_at: time..) }
pub trait ApplicationRecordScopes {
    fn created_since(self, time: Time) -> Self;
}

impl<M: Model> ApplicationRecordScopes for Relation<M> {
    fn created_since(self, time: Time) -> Self {
        self.where_gte("created_at", time)
    }
}

// user.rb
model! {
    pub struct User in "users" {
        id: i64,
        name: String,
        email: String,
        created_at: Time,
        updated_at: Time,
    }
}

impl Model for User {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<User>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

// post.rb
model! {
    pub struct Post in "posts" {
        id: i64,
        user_id: i64,
        title: String,
        body: String,
        status: String = "draft",
        published_at: Time,
        comments_count: i64 = 0,
        created_at: Time,
        updated_at: Time,
    }
}

impl Post {
    // post.rb:5  enum :status gives published? and draft?
    pub fn is_published(&self) -> bool {
        self.status.as_deref() == Some("published")
    }

    pub fn is_draft(&self) -> bool {
        self.status.as_deref() == Some("draft")
    }
}

impl Model for Post {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Post>> =
            LazyLock::new(|| Behavior::new().enumeration("status", &[("draft", 0), ("published", 1)], true));
        &BEHAVIOR
    }
}

pub trait PostScopes {
    fn recent(self) -> Self;
    fn visible(self) -> Self;
}

impl PostScopes for Relation<Post> {
    // post.rb:9  scope :recent, -> { order(created_at: :desc) }
    fn recent(self) -> Self {
        self.order_desc("created_at")
    }

    // post.rb:10  scope :visible, -> { where(status: :published) }
    fn visible(self) -> Self {
        self.where_eq("status", "published")
    }
}

// comment.rb
model! {
    pub struct Comment in "comments" {
        id: i64,
        post_id: i64,
        user_id: i64,
        body: String,
        created_at: Time,
        updated_at: Time,
    }
}

impl Model for Comment {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Comment>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}
```

- [ ] **Step 2: Write the failing relation tests**

Create `tests/relation_test.rs`:

```rust
mod blog;
mod support;

use blog::{ApplicationRecordScopes, Post, PostScopes, User};
use rustonrails::{Ctx, Error, Model, Value, now};

fn user(ctx: &mut Ctx, email: &str) -> i64 {
    let rows = ctx
        .query(
            "INSERT INTO users (name, email, created_at, updated_at) VALUES ('U', $1, now(), now()) RETURNING id",
            &[Value::from(email)],
        )
        .unwrap();
    rows[0].get(0)
}

fn post(ctx: &mut Ctx, user_id: i64, title: &str, status: i64, days_ago: i64) -> i64 {
    let created = now() - chrono::TimeDelta::days(days_ago);
    let rows = ctx
        .query(
            "INSERT INTO posts (user_id, title, status, created_at, updated_at) VALUES ($1, $2, $3, $4, $4) RETURNING id",
            &[Value::Int(user_id), Value::from(title), Value::Int(status), Value::Time(created)],
        )
        .unwrap();
    rows[0].get(0)
}

#[test]
fn test_find_loads_a_record_with_enum_labels() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    let id = post(&mut ctx, alice, "Hello", 1, 0);
    let found = Post::find(&mut ctx, id).unwrap();
    assert_eq!(Some("Hello"), ctx[found].title.as_deref());
    assert_eq!(Some("published"), ctx[found].status.as_deref());
    assert!(ctx.is_persisted(found));
    assert!(ctx.changed(found).is_empty());
}

#[test]
fn test_find_missing_raises_record_not_found() {
    let mut ctx = support::ctx();
    let error = Post::find(&mut ctx, 0).unwrap_err();
    assert!(matches!(error, Error::RecordNotFound { .. }));
    assert_eq!("Couldn't find Post with 'id'=0", error.to_string());
}

#[test]
fn test_find_by_and_bang() {
    let mut ctx = support::ctx();
    user(&mut ctx, "bob@example.com");
    let bob = User::find_by(&mut ctx, "email", "bob@example.com").unwrap().unwrap();
    assert_eq!(Some("bob@example.com"), ctx[bob].email.as_deref());
    assert!(User::find_by(&mut ctx, "email", "nobody@example.com").unwrap().is_none());
    let error = User::find_by_bang(&mut ctx, "email", "nobody@example.com").unwrap_err();
    assert_eq!("Couldn't find User", error.to_string());
}

#[test]
fn test_scopes_chain_like_ruby() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    let old = post(&mut ctx, alice, "Old", 1, 3);
    let new = post(&mut ctx, alice, "New", 1, 1);
    post(&mut ctx, alice, "Draft", 0, 0);
    let visible = Post::all().visible().recent().load(&mut ctx).unwrap();
    let ids: Vec<Option<i64>> = visible.iter().map(|p| ctx[*p].id).collect();
    assert_eq!(vec![Some(new), Some(old)], ids);
    assert_eq!(1, Post::all().visible().recent().limit(1).load(&mut ctx).unwrap().len());
}

#[test]
fn test_inherited_scope_filters_by_time() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "Old", 1, 3);
    let new = post(&mut ctx, alice, "New", 1, 1);
    let since = now() - chrono::TimeDelta::days(2);
    let recent = Post::all().created_since(since).load(&mut ctx).unwrap();
    assert_eq!(vec![Some(new)], recent.iter().map(|p| ctx[*p].id).collect::<Vec<_>>());
}

#[test]
fn test_count_exists_first() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    let first = post(&mut ctx, alice, "A", 0, 0);
    post(&mut ctx, alice, "B", 0, 0);
    assert_eq!(2, Post::all().count(&mut ctx).unwrap());
    assert_eq!(1, Post::all().limit(1).count(&mut ctx).unwrap());
    assert!(Post::all().where_eq("title", "B").exists(&mut ctx).unwrap());
    assert!(!Post::all().where_eq("title", "C").exists(&mut ctx).unwrap());
    let loaded = Post::all().first(&mut ctx).unwrap().unwrap();
    assert_eq!(Some(first), ctx[loaded].id);
}

#[test]
fn test_unknown_enum_label_matches_nothing() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "A", 0, 0);
    assert_eq!(0, Post::all().where_eq("status", "archived").count(&mut ctx).unwrap());
    let (sql, params) = Post::all().where_eq("status", "published").to_sql();
    assert!(sql.contains("\"posts\".\"status\" = $1"), "{sql}");
    assert_eq!(vec![Value::Int(1)], params);
}

#[test]
fn test_values_are_parameters_not_sql() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "It's \"quoted\"; DROP TABLE posts", 0, 0);
    let found = Post::find_by(&mut ctx, "title", "It's \"quoted\"; DROP TABLE posts").unwrap();
    assert!(found.is_some());
    assert_eq!(1, Post::all().count(&mut ctx).unwrap());
}

#[test]
fn test_where_not_and_nil() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "A", 0, 0);
    post(&mut ctx, alice, "B", 1, 0);
    assert_eq!(1, Post::all().where_not("status", "draft").count(&mut ctx).unwrap());
    assert_eq!(2, Post::all().where_eq("published_at", None::<rustonrails::Time>).count(&mut ctx).unwrap());
}
```

Add `chrono` to the test's reach: integration tests can use the crate's dependencies directly, so `chrono::TimeDelta` works as written.

- [ ] **Step 3: Run to see them fail**

Run: `cargo test --test relation_test 2>&1 | tail -3`
Expected: compile errors, `unresolved import rustonrails::Relation` and `no function or associated item named find`.

- [ ] **Step 4: Implement `Relation`**

Create `src/relation.rs`:

```rust
use std::marker::PhantomData;

use postgres::Row;

use crate::pg::{self, quote};
use crate::{Ctx, Error, Handle, Model, Result, Value};

#[derive(Clone, Debug)]
enum Filter {
    Eq(String, Value),
    NotEq(String, Value),
    Gte(String, Value),
}

/// A lazy query, like `ActiveRecord::Relation`: nothing runs until `load`,
/// `first`, `count`, `exists` or a finder.
pub struct Relation<M> {
    filters: Vec<Filter>,
    orders: Vec<(String, &'static str)>,
    limit: Option<i64>,
    marker: PhantomData<fn() -> M>,
}

impl<M> Clone for Relation<M> {
    fn clone(&self) -> Self {
        Self { filters: self.filters.clone(), orders: self.orders.clone(), limit: self.limit, marker: PhantomData }
    }
}

impl<M: Model> Default for Relation<M> {
    fn default() -> Self {
        Self { filters: Vec::new(), orders: Vec::new(), limit: None, marker: PhantomData }
    }
}

impl<M: Model> Relation<M> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `where(column: value)`. Enum columns take the label, as in Rails.
    pub fn where_eq(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.filters.push(Filter::Eq(column.to_string(), value.into()));
        self
    }

    /// `where.not(column: value)`
    pub fn where_not(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.filters.push(Filter::NotEq(column.to_string(), value.into()));
        self
    }

    /// `where(column: value..)`
    pub fn where_gte(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.filters.push(Filter::Gte(column.to_string(), value.into()));
        self
    }

    pub fn order_asc(mut self, column: &str) -> Self {
        self.orders.push((column.to_string(), "ASC"));
        self
    }

    pub fn order_desc(mut self, column: &str) -> Self {
        self.orders.push((column.to_string(), "DESC"));
        self
    }

    pub fn limit(mut self, n: i64) -> Self {
        self.limit = Some(n);
        self
    }

    /// The SELECT this relation runs, with its parameters.
    pub fn to_sql(&self) -> (String, Vec<Value>) {
        let table = quote(M::TABLE);
        let mut sql = format!("SELECT {table}.* FROM {table}");
        let mut params = Vec::new();
        for (i, filter) in self.filters.iter().enumerate() {
            sql.push_str(if i == 0 { " WHERE " } else { " AND " });
            let (column, op, value) = match filter {
                Filter::Eq(c, v) => (c, "=", v),
                Filter::NotEq(c, v) => (c, "<>", v),
                Filter::Gte(c, v) => (c, ">=", v),
            };
            let target = format!("{table}.{}", quote(column));
            match M::behavior().to_database(column, value.clone()) {
                Value::Nil if op == "<>" => sql.push_str(&format!("{target} IS NOT NULL")),
                Value::Nil => sql.push_str(&format!("{target} IS NULL")),
                value => {
                    params.push(value);
                    sql.push_str(&format!("{target} {op} ${}", params.len()));
                }
            }
        }
        if !self.orders.is_empty() {
            let orders: Vec<String> = self.orders.iter().map(|(c, dir)| format!("{table}.{} {dir}", quote(c))).collect();
            sql.push_str(&format!(" ORDER BY {}", orders.join(", ")));
        }
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        (sql, params)
    }

    pub(crate) fn fetch(&self, ctx: &mut Ctx) -> Result<Vec<M>> {
        let (sql, params) = self.to_sql();
        ctx.query(&sql, &params)?.iter().map(from_row::<M>).collect()
    }

    pub fn load(&self, ctx: &mut Ctx) -> Result<Vec<Handle<M>>> {
        Ok(self.fetch(ctx)?.into_iter().map(|record| ctx.adopt(record)).collect())
    }

    /// `first`: orders by id unless the relation has an order already.
    pub fn first(&self, ctx: &mut Ctx) -> Result<Option<Handle<M>>> {
        let mut relation = self.clone().limit(1);
        if relation.orders.is_empty() {
            relation = relation.order_asc("id");
        }
        Ok(relation.load(ctx)?.into_iter().next())
    }

    pub fn count(&self, ctx: &mut Ctx) -> Result<i64> {
        let (sql, params) = self.to_sql();
        let rows = ctx.query(&format!("SELECT COUNT(*) FROM ({sql}) AS subquery"), &params)?;
        Ok(rows[0].get(0))
    }

    pub fn exists(&self, ctx: &mut Ctx) -> Result<bool> {
        Ok(!self.clone().limit(1).fetch(ctx)?.is_empty())
    }

    pub fn find(&self, ctx: &mut Ctx, id: i64) -> Result<Handle<M>> {
        let found = self.clone().where_eq("id", id).limit(1).load(ctx)?.into_iter().next();
        found.ok_or_else(|| Error::RecordNotFound { model: M::NAME, conditions: Some(format!("'id'={id}")) })
    }

    pub fn find_by(&self, ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Option<Handle<M>>> {
        Ok(self.clone().where_eq(column, value).limit(1).load(ctx)?.into_iter().next())
    }

    pub fn find_by_bang(&self, ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Handle<M>> {
        self.find_by(ctx, column, value)?.ok_or(Error::RecordNotFound { model: M::NAME, conditions: None })
    }
}

/// Builds a record from a row, turning enum integers back into labels.
pub(crate) fn from_row<M: Model>(row: &Row) -> Result<M> {
    let mut record = M::default();
    for (index, column) in row.columns().iter().enumerate() {
        let value = M::behavior().from_database(column.name(), pg::read(row, index)?);
        record.set(column.name(), value)?;
    }
    Ok(record)
}
```

In `src/model.rs`, change the `use` line to `use crate::{Behavior, Ctx, Handle, Relation, Result, Value};` and give `Model` its class methods:

```rust
/// A record plus its class-level declarations. Class methods such as
/// `Post.find` are default methods here.
pub trait Model: Record {
    fn behavior() -> &'static Behavior<Self>;

    fn all() -> Relation<Self> {
        Relation::new()
    }

    fn find(ctx: &mut Ctx, id: i64) -> Result<Handle<Self>> {
        Self::all().find(ctx, id)
    }

    fn find_by(ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Option<Handle<Self>>> {
        Self::all().find_by(ctx, column, value)
    }

    fn find_by_bang(ctx: &mut Ctx, column: &str, value: impl Into<Value>) -> Result<Handle<Self>> {
        Self::all().find_by_bang(ctx, column, value)
    }
}
```

In `src/lib.rs`, add `mod relation;` to the module list and `pub use relation::Relation;` to the exports.

- [ ] **Step 5: Run the tests**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|^error'`
Expected: all `ok`; `relation_test` 9 passed.

- [ ] **Step 6: Commit**

```bash
git add src tests
git commit -m "Queries: Relation, finders, enum-aware conditions, blog models and scopes" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Validations

**Files:**
- Modify: `src/validation.rs`, `src/lib.rs`
- Create: `src/persistence.rs`
- Create: `tests/validation_test.rs`

**Interfaces:**
- Consumes: `Behavior::validations`, `Guarded::applies`, `Check`, `Relation::{where_eq, where_not, exists}`, `Ctx::{slot, errors_mut, attribute_changed, query}`.
- Produces: `Ctx::is_valid(Handle<M>) -> Result<bool>`; crate-private `Ctx::run_callbacks(Handle<M>, Event) -> Result<()>` and `validation::run(&mut Ctx, Handle<M>) -> Result<()>`.

- [ ] **Step 1: Write the failing test**

Create `tests/validation_test.rs`. It declares its own models over the blog's tables so each check can be exercised alone:

```rust
mod support;

use std::sync::LazyLock;

use regex::Regex;
use rustonrails::{Behavior, Check, Ctx, Handle, Model, Record, Result, Time, Value, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::new()
                .before_validation(|ctx, person| {
                    let email = ctx[person].email.clone().unwrap_or_default();
                    ctx[person].email = Some(email.trim().to_lowercase());
                    Ok(())
                })
                .validates("name", Check::Presence)
                .validates("name", Check::Length { minimum: Some(2), maximum: Some(5) })
                .validates("email", Check::Uniqueness)
                .validates("email", Check::Format(Regex::new(r"\A[^@\s]+@[^@\s]+\z").unwrap()))
                .unless(|ctx, person| ctx[person].name.as_deref() == Some("skip"))
                .validate(no_bobs)
        });
        &BEHAVIOR
    }
}

fn no_bobs(ctx: &mut Ctx, person: Handle<Person>) -> Result<()> {
    if ctx[person].name.as_deref() == Some("Bob") {
        ctx.errors_mut(person).add("name", "is reserved");
    }
    Ok(())
}

model! {
    pub struct Entry in "posts" { id: i64, user_id: i64, title: String, status: String = "draft" }
}

impl Model for Entry {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Entry>> = LazyLock::new(|| {
            Behavior::new()
                .belongs_to::<Person>("user", "user_id")
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
        });
        &BEHAVIOR
    }
}

fn person(ctx: &mut Ctx, name: &str, email: &str) -> Handle<Person> {
    ctx.build(Person { name: Some(name.into()), email: Some(email.into()), ..Person::new_record() })
}

fn insert_user(ctx: &mut Ctx, email: &str) -> i64 {
    let rows = ctx
        .query(
            "INSERT INTO users (name, email, created_at, updated_at) VALUES ('Taken', $1, now(), now()) RETURNING id",
            &[Value::from(email)],
        )
        .unwrap();
    rows[0].get(0)
}

#[test]
fn test_presence_rejects_nil_and_whitespace() {
    let mut ctx = support::ctx();
    let nobody = ctx.build(Person::new_record());
    assert!(!ctx.is_valid(nobody).unwrap());
    assert!(ctx.errors(nobody).on("name").contains(&"can't be blank"));
    let spaces = person(&mut ctx, "   ", "a@b.c");
    assert!(!ctx.is_valid(spaces).unwrap());
    assert!(ctx.errors(spaces).on("name").contains(&"can't be blank"));
}

#[test]
fn test_length_messages() {
    let mut ctx = support::ctx();
    let short = person(&mut ctx, "A", "a@b.c");
    ctx.is_valid(short).unwrap();
    assert_eq!(vec!["is too short (minimum is 2 characters)"], ctx.errors(short).on("name"));
    let long = person(&mut ctx, "Abcdef", "a@b.c");
    ctx.is_valid(long).unwrap();
    assert_eq!(vec!["is too long (maximum is 5 characters)"], ctx.errors(long).on("name"));
}

#[test]
fn test_before_validation_runs_first_and_uniqueness_queries() {
    let mut ctx = support::ctx();
    insert_user(&mut ctx, "taken@example.com");
    let dup = person(&mut ctx, "Carol", "  TAKEN@example.com ");
    assert!(!ctx.is_valid(dup).unwrap());
    assert_eq!(Some("taken@example.com"), ctx[dup].email.as_deref());
    assert_eq!(vec!["has already been taken"], ctx.errors(dup).on("email"));
}

#[test]
fn test_format_and_unless() {
    let mut ctx = support::ctx();
    let bad = person(&mut ctx, "Dan", "nope");
    ctx.is_valid(bad).unwrap();
    assert_eq!(vec!["is invalid"], ctx.errors(bad).on("email"));
    let skipped = person(&mut ctx, "skip", "nope");
    assert!(ctx.is_valid(skipped).unwrap());
}

#[test]
fn test_custom_validation_and_error_reset() {
    let mut ctx = support::ctx();
    let bob = person(&mut ctx, "Bob", "bob@example.com");
    assert!(!ctx.is_valid(bob).unwrap());
    assert_eq!(vec!["is reserved"], ctx.errors(bob).on("name"));
    ctx[bob].name = Some("Rob".into());
    assert!(ctx.is_valid(bob).unwrap());
    assert!(ctx.errors(bob).is_empty());
}

#[test]
fn test_belongs_to_must_exist_and_enum_inclusion() {
    let mut ctx = support::ctx();
    let orphan = ctx.build(Entry { user_id: Some(0), title: Some("t".into()), status: Some("archived".into()), ..Entry::new_record() });
    assert!(!ctx.is_valid(orphan).unwrap());
    assert_eq!(vec!["must exist"], ctx.errors(orphan).on("user"));
    assert_eq!(vec!["is not included in the list"], ctx.errors(orphan).on("status"));
    let missing = ctx.build(Entry::new_record());
    ctx.is_valid(missing).unwrap();
    assert_eq!(vec!["must exist"], ctx.errors(missing).on("user"));
    let owner = insert_user(&mut ctx, "owner@example.com");
    let fine = ctx.build(Entry { user_id: Some(owner), ..Entry::new_record() });
    assert!(ctx.is_valid(fine).unwrap());
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --test validation_test 2>&1 | tail -3`
Expected: compile error, `no method named is_valid found for struct Ctx`.

- [ ] **Step 3: Implement the validators**

Append to `src/validation.rs`, and change its first line to add imports:

```rust
use crate::behavior::Validation;
use crate::pg::quote;
use crate::{Check, Ctx, Handle, Model, Result, Value};
```

```rust
/// Runs the model's validations in declaration order, adding to `errors`.
pub(crate) fn run<M: Model>(ctx: &mut Ctx, record: Handle<M>) -> Result<()> {
    for entry in &M::behavior().validations {
        if !entry.applies(ctx, record) {
            continue;
        }
        match &entry.item {
            Validation::Custom(hook) => hook(ctx, record)?,
            Validation::Check { attribute, check } => check_one(ctx, record, attribute, check)?,
        }
    }
    Ok(())
}

fn check_one<M: Model>(ctx: &mut Ctx, record: Handle<M>, attribute: &str, check: &Check) -> Result<()> {
    let value = ctx[record].get(attribute);
    let message = match check {
        Check::Presence => value.is_blank().then(|| "can't be blank".to_string()),
        Check::Length { minimum, maximum } => length_message(&value, *minimum, *maximum),
        Check::Format(regex) => (!regex.is_match(&value.to_ruby_string())).then(|| "is invalid".to_string()),
        Check::Inclusion(allowed) => (!allowed.contains(&value)).then(|| "is not included in the list".to_string()),
        Check::Uniqueness => taken(ctx, record, attribute, value)?.then(|| "has already been taken".to_string()),
        Check::Required { foreign_key, table } => missing(ctx, record, foreign_key, table)?.then(|| "must exist".to_string()),
    };
    if let Some(message) = message {
        ctx.errors_mut(record).add(attribute, message);
    }
    Ok(())
}

/// nil counts as length 0, as `nil.to_s.length` does in Rails.
fn length_message(value: &Value, minimum: Option<usize>, maximum: Option<usize>) -> Option<String> {
    let length = value.to_ruby_string().chars().count();
    let unit = |n: usize| if n == 1 { "character" } else { "characters" };
    if let Some(max) = maximum.filter(|max| length > *max) {
        return Some(format!("is too long (maximum is {max} {})", unit(max)));
    }
    minimum.filter(|min| length < *min).map(|min| format!("is too short (minimum is {min} {})", unit(min)))
}

fn taken<M: Model>(ctx: &mut Ctx, record: Handle<M>, attribute: &str, value: Value) -> Result<bool> {
    let mut others = M::all().where_eq(attribute, value);
    if let Some(id) = ctx.slot(record).saved.as_ref().and_then(|saved| saved.id()) {
        others = others.where_not("id", id);
    }
    others.exists(ctx)
}

/// Rails 7.1+ checks a required `belongs_to` only when the foreign key is
/// nil or changed; then the referenced row has to exist.
fn missing<M: Model>(ctx: &mut Ctx, record: Handle<M>, foreign_key: &str, table: &str) -> Result<bool> {
    let key = ctx[record].get(foreign_key);
    if key.is_nil() {
        return Ok(true);
    }
    if !ctx.attribute_changed(record, foreign_key) {
        return Ok(false);
    }
    let sql = format!("SELECT 1 FROM {} WHERE {} = $1 LIMIT 1", quote(table), quote("id"));
    Ok(ctx.query(&sql, &[key])?.is_empty())
}
```

- [ ] **Step 4: Add `is_valid` and callback running**

Create `src/persistence.rs`:

```rust
use crate::{Ctx, Error, Event, Handle, Model, Result, validation};

impl Ctx {
    /// `valid?`: runs the validation callbacks and the validations,
    /// replacing `errors`. A before_validation abort makes it false.
    pub fn is_valid<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        self.errors_mut(record).clear();
        match self.run_callbacks(record, Event::BeforeValidation) {
            Err(Error::Abort) => return Ok(false),
            other => other?,
        }
        validation::run(self, record)?;
        self.run_callbacks(record, Event::AfterValidation)?;
        Ok(self.errors(record).is_empty())
    }

    /// Runs one event's callbacks in declaration order.
    pub(crate) fn run_callbacks<M: Model>(&mut self, record: Handle<M>, event: Event) -> Result<()> {
        for entry in &M::behavior().callbacks {
            let (on, hook) = entry.item;
            if on == event && entry.applies(self, record) {
                hook(self, record)?;
            }
        }
        Ok(())
    }
}
```

In `src/lib.rs`, add `mod persistence;` to the module list.

- [ ] **Step 5: Run the tests**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|^error'`
Expected: all `ok`; `validation_test` 6 passed.

- [ ] **Step 6: Commit**

```bash
git add src tests
git commit -m "Validations: presence, length, format, uniqueness, inclusion, belongs_to, custom" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Saving, with callbacks

**Files:**
- Create: `src/write.rs`
- Modify: `src/persistence.rs`, `src/model.rs`, `src/lib.rs`
- Create: `tests/save_test.rs`

**Interfaces:**
- Consumes: `Ctx::{is_valid, run_callbacks, transaction, changed, slot, slot_mut}`, `Behavior::to_database`, `now`.
- Produces:
  - `Ctx::{save(h) -> Result<bool>, save_bang(h) -> Result<()>, update(h, impl FnOnce(&mut M)) -> Result<bool>, update_bang(h, ...) -> Result<()>}`
  - `Model::{create(ctx, M) -> Result<Handle<M>>, create_bang(ctx, M) -> Result<Handle<M>>}`
  - crate-private `write::{fill_timestamps, insert_row, update_row}`

- [ ] **Step 1: Write the failing test**

Create `tests/save_test.rs`:

```rust
mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Check, Ctx, Error, Handle, Model, Record, Result, Time, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::new()
                .validates("name", Check::Presence)
                .before_save(|ctx, p| log(ctx, p, "before_save"))
                .before_create(|ctx, p| log(ctx, p, "before_create"))
                .after_create(|ctx, p| log(ctx, p, "after_create"))
                .after_save(|ctx, p| log(ctx, p, "after_save"))
                .before_update(|ctx, p| log(ctx, p, "before_update"))
                .after_update(|ctx, p| log(ctx, p, "after_update"))
                .after_create(|ctx, p| if ctx[p].name.as_deref() == Some("explode") { Err(Error::Nil { what: "explode" }) } else { Ok(()) })
                .before_save(|ctx, p| if ctx[p].name.as_deref() == Some("halt") { Err(Error::Abort) } else { Ok(()) })
        });
        &BEHAVIOR
    }
}

model! {
    pub struct Plain in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Plain {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Plain>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

/// Callbacks append to the email so tests can read the order they ran in.
fn log(ctx: &mut Ctx, person: Handle<Person>, step: &str) -> Result<()> {
    let email = ctx[person].email.get_or_insert_with(String::new);
    email.push_str(step);
    email.push(' ');
    Ok(())
}

fn build(ctx: &mut Ctx, name: &str) -> Handle<Person> {
    ctx.build(Person { name: Some(name.into()), ..Person::new_record() })
}

#[test]
fn test_create_runs_callbacks_in_order_and_sets_timestamps() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "Ann");
    assert!(ctx.save(person).unwrap());
    assert!(ctx.is_persisted(person));
    assert!(ctx[person].id.is_some());
    assert_eq!(Some("before_save before_create after_create after_save "), ctx[person].email.as_deref());
    assert_eq!(ctx[person].created_at, ctx[person].updated_at);
    assert!(ctx[person].created_at.is_some());
}

#[test]
fn test_update_writes_only_changes_and_bumps_updated_at() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "Ann");
    ctx.save_bang(person).unwrap();
    let created = ctx[person].updated_at;
    std::thread::sleep(std::time::Duration::from_millis(2));
    ctx[person].email = None;
    assert!(ctx.update(person, |p| p.name = Some("Bea".into())).unwrap());
    assert_eq!(Some("before_save before_update after_update after_save "), ctx[person].email.as_deref());
    assert!(ctx[person].updated_at > created);
    // Rails applies changes before the after callbacks, so their edits stay unsaved.
    assert_eq!(vec!["email"], ctx.changed(person));
    let reloaded = Person::find(&mut ctx, ctx[person].id.unwrap()).unwrap();
    assert_eq!(Some("Bea"), ctx[reloaded].name.as_deref());
}

#[test]
fn test_save_without_changes_skips_the_update() {
    let mut ctx = support::ctx();
    let plain = Plain::create_bang(&mut ctx, Plain { name: Some("Ann".into()), email: Some("ann@example.com".into()), ..Plain::new_record() }).unwrap();
    let stamp = ctx[plain].updated_at;
    ctx.execute("UPDATE users SET name = 'from the database'", &[]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert!(ctx.save(plain).unwrap());
    assert_eq!(stamp, ctx[plain].updated_at);
    let rows = ctx.query("SELECT name FROM users", &[]).unwrap();
    assert_eq!("from the database", rows[0].get::<_, String>(0));
}

#[test]
fn test_invalid_save_returns_false_and_bang_raises() {
    let mut ctx = support::ctx();
    let nameless = ctx.build(Person::new_record());
    assert!(!ctx.save(nameless).unwrap());
    assert!(ctx.is_new_record(nameless));
    let error = ctx.save_bang(nameless).unwrap_err();
    assert_eq!("Validation failed: Name can't be blank", error.to_string());
    let created = Person::create(&mut ctx, Person::new_record()).unwrap();
    assert!(ctx.is_new_record(created));
    assert!(matches!(Person::create_bang(&mut ctx, Person::new_record()), Err(Error::RecordInvalid { .. })));
}

#[test]
fn test_before_callback_abort_halts_the_save() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "halt");
    assert!(!ctx.save(person).unwrap());
    assert!(ctx.is_new_record(person));
    assert!(matches!(ctx.save_bang(person), Err(Error::RecordNotSaved { .. })));
    assert_eq!(0, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_failed_after_create_rolls_back_and_leaves_record_new() {
    let mut ctx = support::ctx();
    let kept = build(&mut ctx, "Kept");
    ctx.save_bang(kept).unwrap();
    let person = build(&mut ctx, "explode");
    assert!(matches!(ctx.save(person), Err(Error::Nil { .. })));
    assert!(ctx.is_new_record(person));
    assert_eq!(None, ctx[person].id);
    assert_eq!(1, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_timestamps_survive_reload() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "Ann");
    ctx.save_bang(person).unwrap();
    let found = Person::find(&mut ctx, ctx[person].id.unwrap()).unwrap();
    assert_eq!(ctx[person].created_at, ctx[found].created_at);
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --test save_test 2>&1 | tail -3`
Expected: compile error, `no method named save found for struct Ctx`.

- [ ] **Step 3: Implement the SQL writes**

Create `src/write.rs`:

```rust
use crate::pg::{self, quote};
use crate::{Ctx, Model, Record, Result, Time, Value};

/// Sets `created_at` / `updated_at` to `time` where they're nil, as Rails
/// does when creating.
pub(crate) fn fill_timestamps<M: Record>(record: &mut M, time: Time) -> Result<()> {
    for column in ["created_at", "updated_at"] {
        if M::COLUMNS.contains(&column) && record.get(column).is_nil() {
            record.set(column, Value::Time(time))?;
        }
    }
    Ok(())
}

/// One INSERT of every column (id only when set); returns the new id.
pub(crate) fn insert_row<M: Model>(ctx: &mut Ctx, record: &M) -> Result<Value> {
    let columns: Vec<&str> = M::COLUMNS.iter().copied().filter(|c| *c != "id" || !record.get("id").is_nil()).collect();
    let params: Vec<Value> = columns.iter().map(|c| M::behavior().to_database(c, record.get(c))).collect();
    let names: Vec<String> = columns.iter().map(|c| quote(c)).collect();
    let placeholders: Vec<String> = (1..=params.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
        quote(M::TABLE),
        names.join(", "),
        placeholders.join(", "),
        quote("id")
    );
    let rows = ctx.query(&sql, &params)?;
    pg::read(&rows[0], 0)
}

/// One UPDATE of the given columns for the row with this id.
pub(crate) fn update_row<M: Model>(ctx: &mut Ctx, id: i64, record: &M, columns: &[&str]) -> Result<()> {
    let mut params: Vec<Value> = columns.iter().map(|c| M::behavior().to_database(c, record.get(c))).collect();
    let sets: Vec<String> = columns.iter().enumerate().map(|(i, c)| format!("{} = ${}", quote(c), i + 1)).collect();
    params.push(Value::Int(id));
    let sql = format!("UPDATE {} SET {} WHERE {} = ${}", quote(M::TABLE), sets.join(", "), quote("id"), params.len());
    ctx.execute(&sql, &params)?;
    Ok(())
}
```

- [ ] **Step 4: Implement `save` and friends**

Add to `src/persistence.rs` (inside the existing `impl Ctx`), and change its `use` line to `use crate::{Ctx, Error, Event, Handle, Model, Record, Result, Value, now, validation, write};`:

```rust
    /// `save`: false when validations fail or a before callback aborts.
    /// Everything runs in a transaction (a savepoint when nested); on
    /// failure the record keeps its unsaved state and id.
    pub fn save<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        let saved_before = self.slot(record).saved.clone();
        let id_before = self[record].get("id");
        let outcome = self.transaction(|ctx| ctx.create_or_update(record));
        match outcome {
            Ok(true) => Ok(true),
            Ok(false) | Err(Error::Abort) => {
                self.restore(record, saved_before, id_before)?;
                Ok(false)
            }
            Err(error) => {
                self.restore(record, saved_before, id_before)?;
                Err(error)
            }
        }
    }

    /// `save!`
    pub fn save_bang<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        if self.save(record)? {
            return Ok(());
        }
        let messages = self.errors(record).full_messages();
        if messages.is_empty() {
            Err(Error::RecordNotSaved { model: M::NAME })
        } else {
            Err(Error::RecordInvalid { model: M::NAME, messages })
        }
    }

    /// `update(attributes)`: assigns in the closure, then saves.
    pub fn update<M: Model>(&mut self, record: Handle<M>, assign: impl FnOnce(&mut M)) -> Result<bool> {
        assign(&mut self[record]);
        self.save(record)
    }

    /// `update!(attributes)`
    pub fn update_bang<M: Model>(&mut self, record: Handle<M>, assign: impl FnOnce(&mut M)) -> Result<()> {
        assign(&mut self[record]);
        self.save_bang(record)
    }

    fn create_or_update<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        if !self.is_valid(record)? {
            return Ok(false);
        }
        self.run_callbacks(record, Event::BeforeSave)?;
        if self.is_new_record(record) {
            self.run_callbacks(record, Event::BeforeCreate)?;
            self.insert(record)?;
            self.run_callbacks(record, Event::AfterCreate)?;
        } else {
            self.run_callbacks(record, Event::BeforeUpdate)?;
            self.write_changes(record)?;
            self.run_callbacks(record, Event::AfterUpdate)?;
        }
        self.run_callbacks(record, Event::AfterSave)?;
        Ok(true)
    }

    fn insert<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        write::fill_timestamps(&mut self[record], now())?;
        let snapshot = self[record].clone();
        let id = write::insert_row(self, &snapshot)?;
        self[record].set("id", id)?;
        let saved = self[record].clone();
        self.slot_mut(record).saved = Some(saved);
        Ok(())
    }

    /// Writes changed columns only; with no changes there's no UPDATE and
    /// `updated_at` stays, as in Rails.
    fn write_changes<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        let mut columns = self.changed(record);
        if columns.is_empty() {
            return Ok(());
        }
        if M::COLUMNS.contains(&"updated_at") && !columns.contains(&"updated_at") {
            self[record].set("updated_at", Value::Time(now()))?;
            columns.push("updated_at");
        }
        let id = self.saved_id(record)?;
        let snapshot = self[record].clone();
        write::update_row(self, id, &snapshot, &columns)?;
        self.slot_mut(record).saved = Some(snapshot);
        Ok(())
    }

    pub(crate) fn saved_id<M: Model>(&self, record: Handle<M>) -> Result<i64> {
        self.slot(record).saved.as_ref().and_then(Record::id).ok_or(Error::NotPersisted { model: M::NAME })
    }

    /// Puts back what a rolled-back save changed: the saved state and id.
    fn restore<M: Model>(&mut self, record: Handle<M>, saved: Option<M>, id: Value) -> Result<()> {
        self.slot_mut(record).saved = saved;
        self[record].set("id", id)
    }
```

In `src/model.rs`, add to `Model`:

```rust
    /// `Post.create(attributes)`: returns the record even when it didn't save.
    fn create(ctx: &mut Ctx, record: Self) -> Result<Handle<Self>> {
        let handle = ctx.build(record);
        ctx.save(handle)?;
        Ok(handle)
    }

    /// `Post.create!(attributes)`
    fn create_bang(ctx: &mut Ctx, record: Self) -> Result<Handle<Self>> {
        let handle = ctx.build(record);
        ctx.save_bang(handle)?;
        Ok(handle)
    }
```

In `src/lib.rs`, add `mod write;` to the module list.

- [ ] **Step 5: Run the tests**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|^error'`
Expected: all `ok`; `save_test` 7 passed.

- [ ] **Step 6: Commit**

```bash
git add src tests
git commit -m "Saving: create and update with callbacks, timestamps, dirty writes and rollback" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Destroy, reload, increment! and insert

**Files:**
- Modify: `src/write.rs`, `src/persistence.rs`, `src/model.rs`
- Create: `tests/lifecycle_test.rs`

**Interfaces:**
- Consumes: Task 5's `write`, `saved_id`, `transaction`, `run_callbacks`; `Relation::fetch`.
- Produces: `Ctx::{destroy(h) -> Result<bool>, destroy_bang(h) -> Result<()>, reload(h) -> Result<()>, increment_bang(h, column, by) -> Result<()>}`; `Model::insert(ctx, M) -> Result<i64>`; crate-private `write::{delete_row, increment_column}`.

- [ ] **Step 1: Write the failing test**

Create `tests/lifecycle_test.rs`:

```rust
mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Ctx, Error, Handle, Model, Record, Time, Value, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::new()
                .before_destroy(|ctx, p| if ctx[p].name.as_deref() == Some("keep") { Err(Error::Abort) } else { Ok(()) })
        });
        &BEHAVIOR
    }
}

model! {
    pub struct Counted in "posts" {
        id: i64, user_id: i64, title: String, body: String, status: i64 = 0, published_at: Time,
        comments_count: i64 = 0, created_at: Time, updated_at: Time,
    }
}

impl Model for Counted {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Counted>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

fn saved(ctx: &mut Ctx, name: &str) -> Handle<Person> {
    Person::create_bang(ctx, Person { name: Some(name.into()), email: Some(format!("{name}@example.com")), ..Person::new_record() }).unwrap()
}

#[test]
fn test_destroy_deletes_the_row() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "ann");
    assert!(ctx.destroy(person).unwrap());
    assert!(ctx.is_destroyed(person));
    assert!(!ctx.is_persisted(person));
    assert_eq!(0, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_before_destroy_abort_keeps_the_row() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "keep");
    assert!(!ctx.destroy(person).unwrap());
    assert!(!ctx.is_destroyed(person));
    assert!(matches!(ctx.destroy_bang(person), Err(Error::RecordNotDestroyed { .. })));
    assert_eq!(1, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_reload_discards_unsaved_changes() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "ann");
    ctx[person].name = Some("changed".into());
    ctx.reload(person).unwrap();
    assert_eq!(Some("ann"), ctx[person].name.as_deref());
    assert!(ctx.changed(person).is_empty());
}

#[test]
fn test_reload_of_a_deleted_row_raises() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "ann");
    ctx.execute("DELETE FROM users", &[]).unwrap();
    assert!(matches!(ctx.reload(person), Err(Error::RecordNotFound { .. })));
}

#[test]
fn test_increment_bang_updates_memory_and_database() {
    let mut ctx = support::ctx();
    let owner = saved(&mut ctx, "ann");
    let user_id = ctx[owner].id;
    let post = Counted::create_bang(&mut ctx, Counted { user_id, title: Some("t".into()), ..Counted::new_record() }).unwrap();
    let stamp = ctx[post].updated_at;
    ctx.increment_bang(post, "comments_count", 1).unwrap();
    assert_eq!(Some(1), ctx[post].comments_count);
    assert!(ctx.changed(post).is_empty());
    ctx.reload(post).unwrap();
    assert_eq!(Some(1), ctx[post].comments_count);
    assert_eq!(stamp, ctx[post].updated_at);
}

#[test]
fn test_increment_bang_on_a_new_record_raises() {
    let mut ctx = support::ctx();
    let post = ctx.build(Counted::new_record());
    assert!(matches!(ctx.increment_bang(post, "comments_count", 1), Err(Error::NotPersisted { .. })));
}

#[test]
fn test_insert_skips_validations_and_callbacks_but_fills_timestamps() {
    let mut ctx = support::ctx();
    let id = Person::insert(&mut ctx, Person { name: Some("keep".into()), email: Some("k@example.com".into()), ..Person::new_record() }).unwrap();
    let rows = ctx.query("SELECT created_at IS NOT NULL FROM users WHERE id = $1", &[Value::Int(id)]).unwrap();
    assert!(rows[0].get::<_, bool>(0));
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --test lifecycle_test 2>&1 | tail -3`
Expected: compile error, `no method named destroy found for struct Ctx`.

- [ ] **Step 3: Implement the writes**

Append to `src/write.rs`:

```rust
pub(crate) fn delete_row<M: Model>(ctx: &mut Ctx, id: i64) -> Result<()> {
    let sql = format!("DELETE FROM {} WHERE {} = $1", quote(M::TABLE), quote("id"));
    ctx.execute(&sql, &[Value::Int(id)])?;
    Ok(())
}

/// `update_counters`: one atomic `column = COALESCE(column, 0) + by`.
pub(crate) fn increment_column<M: Model>(ctx: &mut Ctx, id: i64, column: &str, by: i64) -> Result<()> {
    let name = quote(column);
    let sql = format!("UPDATE {} SET {name} = COALESCE({name}, 0) + $1 WHERE {} = $2", quote(M::TABLE), quote("id"));
    ctx.execute(&sql, &[Value::Int(by), Value::Int(id)])?;
    Ok(())
}
```

- [ ] **Step 4: Implement the lifecycle methods**

Add to the `impl Ctx` in `src/persistence.rs`:

```rust
    /// `destroy`: runs the destroy callbacks and deletes the row. False
    /// when a before_destroy callback aborts.
    pub fn destroy<M: Model>(&mut self, record: Handle<M>) -> Result<bool> {
        let outcome = self.transaction(|ctx| {
            ctx.run_callbacks(record, Event::BeforeDestroy)?;
            if let Ok(id) = ctx.saved_id(record) {
                write::delete_row::<M>(ctx, id)?;
            }
            ctx.slot_mut(record).destroyed = true;
            ctx.run_callbacks(record, Event::AfterDestroy)?;
            Ok(true)
        });
        match outcome {
            Ok(done) => Ok(done),
            Err(error) => {
                self.slot_mut(record).destroyed = false;
                if matches!(error, Error::Abort) { Ok(false) } else { Err(error) }
            }
        }
    }

    /// `destroy!`
    pub fn destroy_bang<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        if self.destroy(record)? { Ok(()) } else { Err(Error::RecordNotDestroyed { model: M::NAME }) }
    }

    /// `reload`: reads the row again, dropping unsaved changes and errors.
    pub fn reload<M: Model>(&mut self, record: Handle<M>) -> Result<()> {
        let id = self.saved_id(record)?;
        let found = M::all().where_eq("id", id).limit(1).fetch(self)?.into_iter().next();
        let fresh = found.ok_or_else(|| Error::RecordNotFound { model: M::NAME, conditions: Some(format!("'id'={id}")) })?;
        let slot = self.slot_mut(record);
        slot.saved = Some(fresh.clone());
        slot.record = fresh;
        slot.errors.clear();
        Ok(())
    }

    /// `increment!(column, by)`: bumps the value in memory and in the row
    /// with one UPDATE; no validations, no callbacks, no `updated_at`.
    pub fn increment_bang<M: Model>(&mut self, record: Handle<M>, column: &'static str, by: i64) -> Result<()> {
        let id = self.saved_id(record)?;
        let current = match self[record].get(column) {
            Value::Int(i) => i,
            _ => 0,
        };
        self[record].set(column, Value::Int(current + by))?;
        write::increment_column::<M>(self, id, column, by)?;
        let value = self[record].get(column);
        if let Some(saved) = self.slot_mut(record).saved.as_mut() {
            saved.set(column, value)?;
        }
        Ok(())
    }
```

In `src/model.rs`, add to `Model`:

```rust
    /// `Post.insert(attributes)`: one INSERT with no validations or
    /// callbacks; timestamps are filled in as Rails does. Returns the id.
    fn insert(ctx: &mut Ctx, mut record: Self) -> Result<i64> {
        crate::write::fill_timestamps(&mut record, crate::now())?;
        match crate::write::insert_row(ctx, &record)? {
            Value::Int(id) => Ok(id),
            other => Err(crate::Error::Cast { expected: "integer id", value: other }),
        }
    }
```

`Relation::fetch` is `pub(crate)` already, so `reload` can call it.

- [ ] **Step 5: Run the tests**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|^error'`
Expected: all `ok`; `lifecycle_test` 7 passed.

- [ ] **Step 6: Commit**

```bash
git add src tests
git commit -m "Lifecycle: destroy, reload, increment! and insert" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The blog port passes the blog's model tests

**Files:**
- Modify: `tests/blog/mod.rs`
- Create: `tests/blog/fixtures.rs`, `tests/blog_models_test.rs`
- Modify: `docs/design.md`, `README.md`

**Interfaces:**
- Consumes: everything above.
- Produces: complete `User`, `Post`, `Comment` behaviors; `Comment::post(ctx, h) -> Result<Option<Handle<Post>>>`; `blog::fixtures::load(&mut Ctx) -> Fixtures` with ids `alice`, `bob`, `published_old`, `published_new`, `draft`, `first`.

- [ ] **Step 1: Write the fixtures**

Create `tests/blog/fixtures.rs`:

```rust
//! ../Rutile/examples/blog/test/fixtures/*.yml, inserted like Rails
//! fixtures: no validations or callbacks.

use chrono::TimeDelta;
use rustonrails::{Ctx, Model, Record, now};

use super::{Comment, Post, User};

pub struct Fixtures {
    pub alice: i64,
    pub bob: i64,
    pub published_old: i64,
    pub published_new: i64,
    pub draft: i64,
    pub first: i64,
}

pub fn load(ctx: &mut Ctx) -> Fixtures {
    let days_ago = |n: i64| Some(now() - TimeDelta::days(n));
    let user = |ctx: &mut Ctx, name: &str, email: &str| {
        User::insert(ctx, User { name: Some(name.into()), email: Some(email.into()), ..User::new_record() }).unwrap()
    };
    let alice = user(ctx, "Alice", "alice@example.com");
    let bob = user(ctx, "Bob", "bob@example.com");
    let post = |ctx: &mut Ctx, record: Post| Post::insert(ctx, record).unwrap();
    let published_old = post(ctx, Post {
        user_id: Some(alice), title: Some("Old news".into()), body: Some("First post".into()),
        status: Some("published".into()), published_at: days_ago(3), created_at: days_ago(3), comments_count: Some(1),
        ..Post::new_record()
    });
    let published_new = post(ctx, Post {
        user_id: Some(bob), title: Some("Fresh news".into()), body: Some("Second post".into()),
        status: Some("published".into()), published_at: days_ago(1), created_at: days_ago(1),
        ..Post::new_record()
    });
    let draft = post(ctx, Post {
        user_id: Some(alice), title: Some("Work in progress".into()), body: Some("Not yet".into()),
        ..Post::new_record()
    });
    let first = Comment::insert(ctx, Comment {
        post_id: Some(published_old), user_id: Some(bob), body: Some("Nice one".into()), ..Comment::new_record()
    })
    .unwrap();
    Fixtures { alice, bob, published_old, published_new, draft, first }
}
```

- [ ] **Step 2: Write the ported tests**

Create `tests/blog_models_test.rs`:

```rust
//! ../Rutile/examples/blog/test/models/*_test.rb, test for test.

mod blog;
mod support;

use blog::fixtures::{self, Fixtures};
use blog::{ApplicationRecordScopes, Comment, Post, PostScopes, User};
use chrono::TimeDelta;
use rustonrails::{Ctx, Model, Record, now};

fn setup() -> (Ctx, Fixtures) {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    (ctx, fx)
}

// user_test.rb
#[test]
fn user_requires_name_and_email() {
    let (mut ctx, _) = setup();
    let user = ctx.build(User::new_record());
    assert!(!ctx.is_valid(user).unwrap());
    assert!(ctx.errors(user).on("name").contains(&"can't be blank"));
    assert!(ctx.errors(user).on("email").contains(&"can't be blank"));
}

#[test]
fn user_normalizes_email_before_validation() {
    let (mut ctx, _) = setup();
    let user = User::create_bang(&mut ctx, User { name: Some("Carol".into()), email: Some("  Carol@Example.COM ".into()), ..User::new_record() }).unwrap();
    assert_eq!(Some("carol@example.com"), ctx[user].email.as_deref());
}

#[test]
fn user_rejects_a_malformed_email() {
    let (mut ctx, _) = setup();
    let user = ctx.build(User { name: Some("Dan".into()), email: Some("not-an-email".into()), ..User::new_record() });
    assert!(!ctx.is_valid(user).unwrap());
    assert!(ctx.errors(user).on("email").contains(&"is invalid"));
}

#[test]
fn user_email_is_unique_after_normalizing() {
    let (mut ctx, _) = setup();
    let user = ctx.build(User { name: Some("Alice 2".into()), email: Some("ALICE@example.com".into()), ..User::new_record() });
    assert!(!ctx.is_valid(user).unwrap());
    assert!(ctx.errors(user).on("email").contains(&"has already been taken"));
}

// post_test.rb
#[test]
fn post_title_is_required_and_at_most_200_characters() {
    let (mut ctx, fx) = setup();
    let post = ctx.build(Post { user_id: Some(fx.alice), title: Some(String::new()), ..Post::new_record() });
    assert!(!ctx.is_valid(post).unwrap());
    assert!(ctx.errors(post).on("title").contains(&"can't be blank"));
    ctx[post].title = Some("x".repeat(201));
    assert!(!ctx.is_valid(post).unwrap());
    assert!(ctx.errors(post).on("title").contains(&"is too long (maximum is 200 characters)"));
}

#[test]
fn post_unknown_status_is_a_validation_error() {
    let (mut ctx, fx) = setup();
    let post = ctx.build(Post { user_id: Some(fx.alice), title: Some("Hi".into()), status: Some("archived".into()), ..Post::new_record() });
    assert!(!ctx.is_valid(post).unwrap());
    assert!(ctx.errors(post).on("status").contains(&"is not included in the list"));
}

#[test]
fn post_publishing_stamps_published_at_once() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.draft).unwrap();
    assert_eq!(None, ctx[post].published_at);
    ctx.update_bang(post, |p| p.status = Some("published".into())).unwrap();
    ctx.reload(post).unwrap();
    let stamped = ctx[post].published_at;
    assert!(stamped.is_some());
    ctx.update_bang(post, |p| p.title = Some("Edited".into())).unwrap();
    ctx.reload(post).unwrap();
    assert_eq!(stamped, ctx[post].published_at);
}

#[test]
fn post_drafts_never_get_published_at() {
    let (mut ctx, fx) = setup();
    let post = Post::create_bang(&mut ctx, Post { user_id: Some(fx.bob), title: Some("Still a draft".into()), ..Post::new_record() }).unwrap();
    assert_eq!(None, ctx[post].published_at);
}

#[test]
fn post_visible_recent_lists_published_posts_newest_first() {
    let (mut ctx, fx) = setup();
    let posts = Post::all().visible().recent().load(&mut ctx).unwrap();
    let ids: Vec<Option<i64>> = posts.iter().map(|p| ctx[*p].id).collect();
    assert_eq!(vec![Some(fx.published_new), Some(fx.published_old)], ids);
}

#[test]
fn post_created_since_filters_by_creation_time() {
    let (mut ctx, fx) = setup();
    let recent = Post::all().created_since(now() - TimeDelta::days(2)).load(&mut ctx).unwrap();
    let ids: Vec<Option<i64>> = recent.iter().map(|p| ctx[*p].id).collect();
    assert!(ids.contains(&Some(fx.published_new)));
    assert!(!ids.contains(&Some(fx.published_old)));
}

// comment_test.rb
#[test]
fn comment_body_is_required() {
    let (mut ctx, fx) = setup();
    let comment = ctx.build(Comment { post_id: Some(fx.published_old), user_id: Some(fx.bob), ..Comment::new_record() });
    assert!(!ctx.is_valid(comment).unwrap());
    assert!(ctx.errors(comment).on("body").contains(&"can't be blank"));
}

#[test]
fn comment_creation_bumps_the_posts_comments_count() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_new).unwrap();
    let before = ctx[post].comments_count.unwrap();
    Comment::create_bang(&mut ctx, Comment { post_id: Some(fx.published_new), user_id: Some(fx.alice), body: Some("Agreed".into()), ..Comment::new_record() }).unwrap();
    ctx.reload(post).unwrap();
    assert_eq!(before + 1, ctx[post].comments_count.unwrap());
}

#[test]
fn comment_posts_must_be_published_before_they_take_comments() {
    let (mut ctx, fx) = setup();
    let comment = ctx.build(Comment { post_id: Some(fx.draft), user_id: Some(fx.bob), body: Some("Early".into()), ..Comment::new_record() });
    assert!(!ctx.is_valid(comment).unwrap());
    assert!(ctx.errors(comment).on("post").contains(&"must be published"));
}
```

- [ ] **Step 3: Run to see them fail**

Run: `cargo test --test blog_models_test 2>&1 | tail -4`
Expected: compile error, `could not find fixtures in blog` (then, once it compiles, failures such as `user_requires_name_and_email` panicking because the blog behaviors are still empty).

- [ ] **Step 4: Complete the blog models**

In `tests/blog/mod.rs`:

1. Change the imports to:

```rust
pub mod fixtures;

use std::sync::LazyLock;

use regex::Regex;
use rustonrails::{Behavior, Check, Ctx, Error, Handle, Model, Relation, Result, Time, model, now};
```

2. Replace `impl Model for User` with:

```rust
/// URI::MailTo::EMAIL_REGEXP, verbatim.
const EMAIL_REGEXP: &str = r"\A[a-zA-Z0-9.!\#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z";

impl Model for User {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<User>> = LazyLock::new(|| {
            Behavior::new()
                // user.rb:5  before_validation { self.email = email.to_s.strip.downcase }
                .before_validation(|ctx, user| {
                    let email = ctx[user].email.clone().unwrap_or_default();
                    ctx[user].email = Some(email.trim().to_lowercase());
                    Ok(())
                })
                // user.rb:7
                .validates("name", Check::Presence)
                // user.rb:8
                .validates("email", Check::Presence)
                .validates("email", Check::Uniqueness)
                .validates("email", Check::Format(Regex::new(EMAIL_REGEXP).expect("EMAIL_REGEXP compiles")))
        });
        &BEHAVIOR
    }
}
```

3. Add to `impl Post` and replace `impl Model for Post`:

```rust
    // post.rb:16
    fn stamp_published_at(ctx: &mut Ctx, post: Handle<Post>) -> Result<()> {
        let post = &mut ctx[post];
        if post.published_at.is_none() {
            post.published_at = Some(now());
        }
        Ok(())
    }
```

```rust
impl Model for Post {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Post>> = LazyLock::new(|| {
            Behavior::new()
                // post.rb:2
                .belongs_to::<User>("user", "user_id")
                // post.rb:5
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
                // post.rb:7
                .validates("title", Check::Presence)
                .validates("title", Check::Length { minimum: None, maximum: Some(200) })
                // post.rb:12
                .before_save(Post::stamp_published_at)
                .when(|ctx, post| ctx[post].is_published())
        });
        &BEHAVIOR
    }
}
```

4. Replace `impl Model for Comment` with:

```rust
impl Comment {
    /// `comment.post` (belongs_to :post). Not cached yet; see docs/design.md.
    pub fn post(ctx: &mut Ctx, comment: Handle<Comment>) -> Result<Option<Handle<Post>>> {
        match ctx[comment].post_id {
            Some(id) => Post::find_by(ctx, "id", id),
            None => Ok(None),
        }
    }

    // comment.rb:12
    fn post_is_published(ctx: &mut Ctx, comment: Handle<Comment>) -> Result<()> {
        if let Some(post) = Comment::post(ctx, comment)? {
            if ctx[post].is_draft() {
                ctx.errors_mut(comment).add("post", "must be published");
            }
        }
        Ok(())
    }

    // comment.rb:16  post.increment!(:comments_count)
    fn bump_post_counter(ctx: &mut Ctx, comment: Handle<Comment>) -> Result<()> {
        let post = Comment::post(ctx, comment)?.ok_or(Error::Nil { what: "increment!" })?;
        ctx.increment_bang(post, "comments_count", 1)
    }
}

impl Model for Comment {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Comment>> = LazyLock::new(|| {
            Behavior::new()
                // comment.rb:2-3
                .belongs_to::<Post>("post", "post_id")
                .belongs_to::<User>("user", "user_id")
                // comment.rb:5
                .validates("body", Check::Presence)
                .validates("body", Check::Length { minimum: None, maximum: Some(2000) })
                // comment.rb:6
                .validate(Comment::post_is_published)
                // comment.rb:8
                .after_create(Comment::bump_post_counter)
        });
        &BEHAVIOR
    }
}
```

- [ ] **Step 5: Run everything**

Run: `cargo test 2>&1 | grep -E '^test result|panicked|^error'`
Expected: all `ok`; `blog_models_test` 13 passed.

Run: `cargo clippy --all-targets 2>&1 | grep -E '^error' | head`
Expected: no output.

Run: `wc -l src/*.rs | sort -n | tail -3`
Expected: no file over 200 lines.

- [ ] **Step 6: Document**

In `docs/design.md`, under `## Records`, append:

````markdown
The port of `examples/blog`'s models in `tests/blog/` is the reference for what `rutile build` should emit, and `tests/blog_models_test.rs` runs the blog's model tests against it:

```rust
let post = Post::find(ctx, id)?;                          // Post.find(id)
ctx.update_bang(post, |p| p.status = Some("published".into()))?; // post.update!(status: :published)
let posts = Post::all().visible().recent().load(ctx)?;    // Post.visible.recent
```
````

In `README.md`, replace the line starting with `**Status:**` with:

```markdown
**Status:** the record layer works (started 2026-09-25): `model!` structs, queries, validations, callbacks and persistence, verified by running `examples/blog`'s model tests against a Rust port in `tests/blog/`. Controllers, routing and JSON come next.

Tests need Postgres: start the Rutile repo's cluster with `cd ../Rutile && bundle exec rake pg:start`, or point `RUSTONRAILS_TEST_DATABASE_URL` at your own.
```

- [ ] **Step 7: Commit**

```bash
git add tests docs/design.md README.md
git commit -m "Blog port: models and fixtures pass the blog's model tests" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
