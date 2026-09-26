//! Ruby's operators on a `Value` whose class is only known at run time:
//! what generated code calls where no static type reached it (Rutile's
//! `Value` fallback). Each gives Ruby's result or fails where Ruby raises,
//! with Ruby's message.

use std::cmp::Ordering;

use chrono::{Duration, TimeDelta};

use crate::cast::ruby_float;
use crate::json::format_date;
use crate::{Error, Result, Value};

/// Past this many bytes `String#*` fails rather than try to allocate:
/// Ruby would raise NoMemoryError; Rust would abort the process.
const LARGEST_STRING: usize = 1 << 30;

impl Value {
    /// The class Ruby's errors name.
    pub fn class_name(&self) -> &'static str {
        match self {
            Value::Nil => "NilClass",
            Value::Bool(true) => "TrueClass",
            Value::Bool(false) => "FalseClass",
            Value::Int(_) => "Integer",
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::Time(_) => "ActiveSupport::TimeWithZone",
            Value::Date(_) => "Date",
        }
    }

    /// Ruby's truth: only nil and false are false.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// `to_s`: nil is "", a Float is written as Ruby writes it, a time as
    /// `ActiveSupport::TimeWithZone#to_s` in UTC.
    pub fn to_s(&self) -> String {
        match self {
            Value::Time(t) => t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            Value::Date(d) => format_date(*d),
            other => other.to_ruby_string(),
        }
    }

    /// `to_f`: nil is 0.0, a String's leading number.
    pub fn to_f(&self) -> Result<f64> {
        match self {
            Value::Nil => Ok(0.0),
            Value::Int(i) => Ok(*i as f64),
            Value::Float(f) => Ok(*f),
            Value::Str(s) => Ok(crate::cast::to_f(s)),
            Value::Time(t) => Ok(t.and_utc().timestamp_micros() as f64 / 1e6),
            other => Err(no_method("to_f", other)),
        }
    }

    /// `==`: an Integer equals the Float of the same number exactly;
    /// values of other classes are never equal.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => int_float(*a, *b) == Some(Ordering::Equal),
            (a, b) => a == b,
        }
    }

    /// `<`, `<=`, `>` or `>=` (`op`).
    pub fn compare(&self, op: &'static str, other: &Value) -> Result<bool> {
        let order = match (self, other) {
            (Value::Nil | Value::Bool(_), _) => return Err(no_method(op, self)),
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Int(a), Value::Float(b)) => int_float(*a, *b),
            (Value::Float(a), Value::Int(b)) => int_float(*b, *a).map(Ordering::reverse),
            (Value::Str(a), Value::Str(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
            (Value::Time(a), Value::Time(b)) => Some(a.cmp(b)),
            (Value::Date(a), Value::Date(b)) => Some(a.cmp(b)),
            _ => {
                let message = format!("comparison of {} with {} failed", self.class_name(), compared_name(other));
                return Err(Error::Argument { message });
            }
        };
        // NaN compares false every way, as in Ruby.
        Ok(order.is_some_and(|order| match op {
            "<" => order == Ordering::Less,
            "<=" => order != Ordering::Greater,
            ">" => order == Ordering::Greater,
            _ => order != Ordering::Less,
        }))
    }

    /// `+`
    pub fn add(&self, other: &Value) -> Result<Value> {
        match (self, other) {
            (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{a}{b}"))),
            (Value::Str(_), other) => Err(Error::Type { message: format!("no implicit conversion of {} into String", coerced_name(other)) }),
            (Value::Time(t), Value::Int(_) | Value::Float(_)) => Ok(Value::Time(*t + seconds(other)?)),
            (Value::Date(d), Value::Int(n)) => Ok(Value::Date(*d + Duration::days(*n))),
            _ => self.arithmetic("+", other, i64::checked_add, |a, b| a + b),
        }
    }

    /// `-`
    pub fn sub(&self, other: &Value) -> Result<Value> {
        match (self, other) {
            (Value::Time(a), Value::Time(b)) => Ok(Value::Float((*a - *b).num_microseconds().unwrap_or(i64::MAX) as f64 / 1e6)),
            (Value::Time(t), Value::Int(_) | Value::Float(_)) => Ok(Value::Time(*t - seconds(other)?)),
            (Value::Date(d), Value::Int(n)) => Ok(Value::Date(*d - Duration::days(*n))),
            (Value::Str(_), _) => Err(no_method("-", self)),
            _ => self.arithmetic("-", other, i64::checked_sub, |a, b| a - b),
        }
    }

    /// `*`: a String repeated, or numbers multiplied.
    pub fn mul(&self, other: &Value) -> Result<Value> {
        let times = match (self, other) {
            (Value::Str(_), Value::Int(n)) => Some(*n),
            (Value::Str(_), Value::Float(f)) => Some(f.trunc() as i64),
            (Value::Str(_), other) => {
                return Err(Error::Type { message: format!("no implicit conversion of {} into Integer", coerced_name(other)) });
            }
            _ => None,
        };
        match (self, times) {
            (Value::Str(_), Some(n)) if n < 0 => Err(Error::Argument { message: "negative argument".into() }),
            (Value::Str(s), Some(n)) => {
                let size = usize::try_from(n).ok().and_then(|n| s.len().checked_mul(n));
                match size {
                    Some(size) if size <= LARGEST_STRING => Ok(Value::Str(s.repeat(n as usize))),
                    _ => Err(Error::Argument { message: "argument too big".into() }),
                }
            }
            _ => self.arithmetic("*", other, i64::checked_mul, |a, b| a * b),
        }
    }

    /// `/`: Integers round toward negative infinity; with a Float either
    /// side it's a Float division, by zero too.
    pub fn div(&self, other: &Value) -> Result<Value> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(crate::div_integers(*a, *b)?)),
            (Value::Str(_), _) => Err(no_method("/", self)),
            _ => self.arithmetic("/", other, |_, _| None, |a, b| a / b),
        }
    }

    /// `%`: the remainder takes the divisor's sign. A String's `%` is
    /// `format`, which the fallback doesn't do.
    pub fn modulo(&self, other: &Value) -> Result<Value> {
        let float = |value: &Value| match value {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        };
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(crate::mod_integers(*a, *b)?)),
            (Value::Int(_) | Value::Float(_), _) if float(other).is_some() => {
                Ok(Value::Float(crate::mod_floats(float(self).unwrap_or_default(), float(other).unwrap_or_default())?))
            }
            (Value::Str(_), _) => Err(Error::Type { message: "String#% (format) isn't supported by the Value fallback".into() }),
            _ => self.arithmetic("%", other, |_, _| None, |a, b| a % b),
        }
    }

    /// Numbers: Integers stay Integers (past 64 bits Ruby makes a Bignum,
    /// which fails here); with a Float either side, a Float.
    fn arithmetic(
        &self,
        op: &'static str,
        other: &Value,
        ints: fn(i64, i64) -> Option<i64>,
        floats: fn(f64, f64) -> f64,
    ) -> Result<Value> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => ints(*a, *b).map(Value::Int).ok_or_else(|| Error::Overflow { value: format!("{a} {op} {b}") }),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(floats(*a as f64, *b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(floats(*a, *b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(floats(*a, *b))),
            (Value::Int(_) | Value::Float(_), other) => {
                Err(Error::Type { message: format!("{} can't be coerced into {}", coerced_name(other), self.class_name()) })
            }
            (Value::Time(_), other) => Err(Error::Type { message: format!("can't convert {} into an exact number", other.class_name()) }),
            (Value::Date(_), _) => Err(Error::Type { message: "expected numeric".into() }),
            _ => Err(no_method(op, self)),
        }
    }
}

/// Ruby compares an Integer with a Float exactly, not through a Float.
fn int_float(a: i64, b: f64) -> Option<Ordering> {
    if b.is_nan() {
        return None;
    }
    // 2^63 as an f64; i64::MAX rounds up to it.
    const TOP: f64 = 9_223_372_036_854_775_808.0;
    if b >= TOP {
        return Some(Ordering::Less);
    }
    if b < -TOP {
        return Some(Ordering::Greater);
    }
    let whole = b.trunc();
    match a.cmp(&(whole as i64)) {
        Ordering::Equal => 0.0_f64.partial_cmp(&(b - whole)),
        order => Some(order),
    }
}

/// A time plus seconds, to the microsecond the database keeps.
fn seconds(amount: &Value) -> Result<TimeDelta> {
    let micros = match amount {
        Value::Int(n) => n.checked_mul(1_000_000),
        Value::Float(f) if f.is_finite() && f.abs() < 9.2e12 => Some((f * 1e6).round() as i64),
        _ => None,
    };
    micros.map(TimeDelta::microseconds).ok_or_else(|| Error::Argument { message: "time out of range".into() })
}

/// How a TypeError names what couldn't be coerced: nil, true and false by
/// name, anything else by class.
fn coerced_name(value: &Value) -> String {
    match value {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        other => other.class_name().into(),
    }
}

/// How a failed comparison names the other side: Ruby inspects nil, true,
/// false and numbers, and names other objects by class.
fn compared_name(value: &Value) -> String {
    match value {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => ruby_float(*f),
        other => other.class_name().into(),
    }
}

fn no_method(what: &'static str, value: &Value) -> Error {
    match value {
        Value::Nil => Error::Nil { what },
        other => Error::NoMethod { what, value: other.clone() },
    }
}
