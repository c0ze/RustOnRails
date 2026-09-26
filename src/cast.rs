use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use regex::Regex;

use crate::{Date, Error, Result, Time, Value};

/// Casts an assigned value to an attribute's type the way Active Model
/// types do (checked against Rails 8.1.4): "42" becomes 42, "abc" becomes 0
/// as `to_i` would, and a blank string becomes nil.
pub trait FromValue: Sized {
    fn from_value(value: Value) -> Result<Option<Self>>;

    /// A query value cast by the column's type, as Active Model's
    /// `serialize` does: anything that doesn't fit becomes nil.
    fn serialize(value: Value) -> Option<Self> {
        Self::from_value(value).ok().flatten()
    }

    /// What `where` binds for a column of this type: `serialize`'s value,
    /// or nil.
    fn query(value: Value) -> Value
    where
        Self: Into<Value>,
    {
        Self::serialize(value).map_or(Value::Nil, Into::into)
    }
}

static LEADING_INTEGER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[+-]?\d+").expect("regex"));
static LEADING_FLOAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[+-]?(\d+(\.\d+)?|\.\d+)([eE][+-]?\d+)?").expect("regex"));

/// Ruby's `String#to_i`: the leading integer, 0 when there is none.
fn to_i(s: &str) -> i64 {
    LEADING_INTEGER.find(s).and_then(|m| m.as_str().trim().parse().ok()).unwrap_or(0)
}

/// Ruby's `String#to_f`: the leading number, 0.0 when there is none.
pub(crate) fn to_f(s: &str) -> f64 {
    LEADING_FLOAT.find(s).and_then(|m| m.as_str().trim().parse().ok()).unwrap_or(0.0)
}

impl FromValue for i64 {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Int(i) => Ok(Some(i)),
            Value::Float(f) => Ok(Some(f as i64)),
            Value::Bool(b) => Ok(Some(b.into())),
            Value::Str(s) if s.trim().is_empty() => Ok(None),
            Value::Str(s) => Ok(Some(to_i(&s))),
            other => Err(Error::Cast { expected: "integer", value: other }),
        }
    }

    /// Unlike assignment, a query value with no leading digits is nil, not 0.
    fn serialize(value: Value) -> Option<Self> {
        match value {
            Value::Str(s) if !LEADING_INTEGER.is_match(&s) => None,
            other => Self::from_value(other).ok().flatten(),
        }
    }
}

impl FromValue for f64 {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Float(f) => Ok(Some(f)),
            Value::Int(i) => Ok(Some(i as f64)),
            Value::Str(s) if s.trim().is_empty() => Ok(None),
            Value::Str(s) => Ok(Some(to_f(&s))),
            other => Err(Error::Cast { expected: "float", value: other }),
        }
    }

    fn serialize(value: Value) -> Option<Self> {
        match value {
            Value::Str(s) if !LEADING_FLOAT.is_match(&s) => None,
            other => Self::from_value(other).ok().flatten(),
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
            // Active Model casts a Date to its midnight.
            Value::Date(d) => Ok(Some(d.and_time(chrono::NaiveTime::MIN))),
            Value::Str(s) => Ok(parse_time(s.trim())),
            other => Err(Error::Cast { expected: "datetime", value: other }),
        }
    }
}

/// The datetime strings Rails accepts in practice: ISO 8601 with or without
/// an offset (converted to UTC), a space instead of `T`, or a bare date
/// (midnight). Anything else is nil, as in Rails.
fn parse_time(s: &str) -> Option<Time> {
    if let Ok(time) = DateTime::parse_from_rfc3339(s) {
        return Some(time.naive_utc());
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f %z", "%Y-%m-%d %H:%M:%S%.f%:z"] {
        if let Ok(time) = DateTime::parse_from_str(s, format) {
            return Some(time.naive_utc());
        }
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(time) = NaiveDateTime::parse_from_str(s, format) {
            return Some(time);
        }
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|date| date.and_hms_opt(0, 0, 0))
}

impl FromValue for Date {
    fn from_value(value: Value) -> Result<Option<Self>> {
        match value {
            Value::Nil => Ok(None),
            Value::Date(d) => Ok(Some(d)),
            Value::Time(t) => Ok(Some(t.date())),
            Value::Str(s) => parse_date(&s),
            other => Err(Error::Cast { expected: "date", value: other }),
        }
    }

    /// A string in a format only `Date._parse` reads stays a string, so
    /// binding it to the date column fails: Rails would find a date there,
    /// and nil would match the NULL rows instead.
    fn query(value: Value) -> Value {
        match Self::from_value(value.clone()) {
            Ok(date) => date.map_or(Value::Nil, Value::Date),
            Err(_) if matches!(value, Value::Str(_)) => value,
            Err(_) => Value::Nil,
        }
    }
}

/// An ISO 8601 date, alone or starting a timestamp, whose offset Rails
/// ignores too. `\d` would take any Unicode digit.
static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| {
    let time = r"[Tt ] *(?:[0-9]{2}:[0-9]{2}(?::[0-9]{2}(?:\.[0-9]+)?)?(?: ?(?:[Zz]|UTC|[+-][0-9]{2}(?::?[0-9]{2})?))?)?";
    Regex::new(&format!(r"^([0-9]{{4}})-(?:([0-9]{{1,2}})-([0-9]{{1,2}})|([0-9]{{2}})-([0-9]{{2}}){time})$")).expect("regex")
});

/// What Active Model's date type makes of a string. Rails tries ISO first,
/// then `Date._parse`, which reads many more formats; a string with digits
/// in a format this doesn't know is an error rather than a guess. Without
/// digits there's no year, which is nil in Rails too.
fn parse_date(s: &str) -> Result<Option<Date>> {
    let trimmed = s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    let Some(parts) = ISO_DATE.captures(trimmed) else {
        if trimmed.chars().any(|c| c.is_ascii_digit()) {
            return Err(Error::Cast { expected: "date", value: Value::Str(s.to_string()) });
        }
        return Ok(None);
    };
    // Month and day come from the date-only or the timestamp alternative.
    let field = |alone: usize, timed: usize| {
        parts.get(alone).or_else(|| parts.get(timed)).map_or(0, |m| m.as_str().parse::<u32>().unwrap_or(0))
    };
    let (year, month, day) = (parts[1].parse::<i32>().unwrap_or(0), field(2, 4), field(3, 5));
    // Before 1583 Ruby's dates are Julian, so a few days exist in only one
    // of the calendars.
    let julian_only = year < 1583 && month == 2 && day == 29 && year % 100 == 0 && year % 400 != 0;
    let gregorian_only = year == 1582 && month == 10 && (5..=14).contains(&day);
    if julian_only || gregorian_only {
        return Err(Error::Cast { expected: "date", value: Value::Str(s.to_string()) });
    }
    Ok(NaiveDate::from_ymd_opt(year, month, day))
}

/// Ruby's `Float#to_s`: shortest digits, always a decimal point, and
/// scientific notation outside 1e-4..1e16 ("1.0", "1.0e+20").
pub(crate) fn ruby_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let scientific = format!("{:e}", f.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} has an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point: i32 = exponent.parse::<i32>().expect("integer exponent") + 1;
    let body = if f == 0.0 {
        "0.0".to_string()
    } else if (1..=16).contains(&point) {
        let point = point as usize;
        let int = format!("{digits:0<point$}");
        let frac = digits.get(point..).filter(|d| !d.is_empty()).unwrap_or("0");
        format!("{}.{frac}", &int[..point])
    } else if (-3..=0).contains(&point) {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let rest = if digits.len() > 1 { &digits[1..] } else { "0" };
        format!("{}.{rest}e{:+03}", &digits[..1], point - 1)
    };
    format!("{sign}{body}")
}
