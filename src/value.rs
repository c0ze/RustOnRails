use chrono::{NaiveDateTime, SubsecRound, Utc};

use crate::cast::ruby_float;

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
            Value::Float(f) => ruby_float(*f),
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
