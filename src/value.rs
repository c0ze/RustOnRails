use chrono::{Local, NaiveDate, NaiveDateTime, SubsecRound, Utc};

use crate::cast::ruby_float;
use crate::json::format_date;
use crate::{Error, Result};

/// Times are UTC without a zone, the way Rails stores `datetime` columns.
pub type Time = NaiveDateTime;

/// A `date` column's value.
pub type Date = NaiveDate;

/// `Time.current`, rounded to microseconds like a Rails `datetime(6)`
/// attribute, so it compares equal after a trip through the database.
pub fn now() -> Time {
    Utc::now().naive_utc().trunc_subsecs(6)
}

/// `Date.current`: today in UTC, the zone `now` assumes.
pub fn today() -> Date {
    Utc::now().date_naive()
}

/// `Date.today`: the machine's local date, which is what Ruby reads, not
/// the app's time zone.
pub fn local_today() -> Date {
    Local::now().date_naive()
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
    Date(Date),
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
            Value::Date(d) => format_date(*d),
        }
    }

    /// Ruby's `to_i`: nil is 0, a Float truncates, a Time is its epoch
    /// seconds, and a String reads its leading integer ("42abc" is 42,
    /// "abc" is 0). Where Ruby would make a Bignum this fails, and true and
    /// false have no `to_i`.
    pub fn to_i(&self) -> Result<i64> {
        match self {
            Value::Nil => Ok(0),
            Value::Int(i) => Ok(*i),
            Value::Float(f) => {
                let whole = f.trunc();
                // i64::MAX as f64 rounds up to 2^63, which doesn't fit.
                if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&whole) {
                    Ok(whole as i64)
                } else {
                    Err(Error::Overflow { value: f.to_string() })
                }
            }
            Value::Str(s) => string_to_i(s),
            Value::Time(t) => Ok(t.and_utc().timestamp()),
            // Ruby's Date has no to_i.
            Value::Bool(_) | Value::Date(_) => Err(Error::NoMethod { what: "to_i", value: self.clone() }),
        }
    }

    /// `to_str`: only a String has it, so a value that must be one fails
    /// the way the String method it reaches would in Ruby.
    pub fn to_str(&self) -> Result<String> {
        match self {
            Value::Str(s) => Ok(s.clone()),
            Value::Nil => Err(Error::Nil { what: "to_str" }),
            other => Err(Error::NoMethod { what: "to_str", value: other.clone() }),
        }
    }
}

/// Ruby's `String#to_i`: leading whitespace, a sign, then digits with
/// single underscores between them; whatever follows is ignored.
fn string_to_i(s: &str) -> Result<i64> {
    let s = s.trim_start_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => ("-", &s[1..]),
        Some(b'+') => ("", &s[1..]),
        _ => ("", s),
    };
    let mut digits = String::new();
    let mut underscore = false;
    for c in rest.chars() {
        match c {
            '0'..='9' => {
                digits.push(c);
                underscore = false;
            }
            '_' if !digits.is_empty() && !underscore => underscore = true,
            _ => break,
        }
    }
    if digits.is_empty() {
        return Ok(0);
    }
    let text = format!("{sign}{digits}");
    text.parse().map_err(|_| Error::Overflow { value: text })
}

impl From<bool> for Value { fn from(v: bool) -> Self { Value::Bool(v) } }
impl From<i32> for Value { fn from(v: i32) -> Self { Value::Int(v.into()) } }
impl From<i64> for Value { fn from(v: i64) -> Self { Value::Int(v) } }
impl From<f64> for Value { fn from(v: f64) -> Self { Value::Float(v) } }
impl From<&str> for Value { fn from(v: &str) -> Self { Value::Str(v.to_string()) } }
impl From<String> for Value { fn from(v: String) -> Self { Value::Str(v) } }
impl From<Time> for Value { fn from(v: Time) -> Self { Value::Time(v) } }
impl From<Date> for Value { fn from(v: Date) -> Self { Value::Date(v) } }

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Nil, Into::into)
    }
}
