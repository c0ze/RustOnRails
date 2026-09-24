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
