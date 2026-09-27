//! Core Ruby and ActiveSupport behavior that generated code calls where
//! Rust's nearest method differs from Ruby's.

use serde_json::Value as Json;

use crate::{Error, Result, Value};

/// `Array#sum` of Integers, from `start`. A nil element raises, as
/// Ruby's TypeError; where Ruby would end in a Bignum this fails.
pub fn sum_integers<V: Into<Option<i64>>>(start: i64, values: impl IntoIterator<Item = V>) -> Result<i64> {
    let mut sum = i128::from(start);
    for value in values {
        sum += i128::from(value.into().ok_or(Error::NilCoerced { into: "Integer" })?);
    }
    i64::try_from(sum).map_err(|_| Error::Overflow { value: sum.to_string() })
}

/// `Array#sum` from a Float `start`: each element added in turn, as Ruby
/// does from a Float (from the Integer 0 it compensates rounding instead).
pub fn sum_floats<V: Into<Option<f64>>>(start: f64, values: impl IntoIterator<Item = V>) -> Result<f64> {
    let mut sum = start;
    for value in values {
        sum += value.into().ok_or(Error::NilCoerced { into: "Float" })?;
    }
    Ok(sum)
}

/// `Integer#/`: rounds toward negative infinity, as Ruby does.
pub fn div_integers(a: i64, b: i64) -> Result<i64> {
    if b == 0 {
        return Err(Error::ZeroDivision);
    }
    let quotient = a.checked_div(b).ok_or_else(|| Error::Overflow { value: format!("{a} / {b}") })?;
    Ok(if a % b != 0 && ((a < 0) != (b < 0)) { quotient - 1 } else { quotient })
}

/// `Integer#%`: the remainder takes the divisor's sign.
pub fn mod_integers(a: i64, b: i64) -> Result<i64> {
    if b == 0 {
        return Err(Error::ZeroDivision);
    }
    // i64::MIN % -1 overflows in Rust; it's 0 in Ruby.
    let remainder = a.checked_rem(b).unwrap_or(0);
    Ok(if remainder != 0 && ((remainder < 0) != (b < 0)) { remainder + b } else { remainder })
}

/// `Float#%`, as Ruby's `flodivmod` computes it: the divisor's sign, a
/// zero divisor raising, an infinite one leaving a finite dividend.
pub fn mod_floats(x: f64, y: f64) -> Result<f64> {
    if y.is_nan() {
        return Ok(y);
    }
    if y == 0.0 {
        return Err(Error::ZeroDivision);
    }
    let mut modulo = if x == 0.0 || (y.is_infinite() && !x.is_infinite()) { x } else { x % y };
    if y * modulo < 0.0 {
        modulo += y;
    }
    Ok(modulo)
}

/// Ruby's `String#strip`, `#downcase` and `#upcase`. Rust's `trim` also
/// strips non-breaking and other Unicode spaces, and `to_lowercase` turns a
/// final Σ into ς; Ruby does neither.
pub trait RubyString {
    fn strip(&self) -> String;
    fn downcase(&self) -> String;
    fn upcase(&self) -> String;
}

impl RubyString for str {
    /// Whitespace here is Ruby's: NUL, tab, LF, VT, FF, CR and space.
    fn strip(&self) -> String {
        self.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\0')).to_string()
    }

    fn downcase(&self) -> String {
        self.chars().flat_map(char::to_lowercase).collect()
    }

    fn upcase(&self) -> String {
        self.to_uppercase()
    }
}

/// ActiveSupport's `blank?` and `present?`.
pub trait Blank {
    fn is_blank(&self) -> bool;

    fn is_present(&self) -> bool {
        !self.is_blank()
    }
}

impl Blank for str {
    /// Empty or only whitespace: `/\A[[:space:]]*\z/`.
    fn is_blank(&self) -> bool {
        self.chars().all(char::is_whitespace)
    }
}

impl Blank for String {
    fn is_blank(&self) -> bool {
        self.as_str().is_blank()
    }
}

impl Blank for Json {
    fn is_blank(&self) -> bool {
        match self {
            Json::Null | Json::Bool(false) => true,
            Json::String(s) => s.is_blank(),
            Json::Array(items) => items.is_empty(),
            Json::Object(map) => map.is_empty(),
            Json::Bool(true) | Json::Number(_) => false,
        }
    }
}

impl Blank for Value {
    fn is_blank(&self) -> bool {
        Value::is_blank(self)
    }
}

impl<T: Blank + ?Sized> Blank for &T {
    fn is_blank(&self) -> bool {
        (**self).is_blank()
    }
}

impl<T: Blank> Blank for Option<T> {
    fn is_blank(&self) -> bool {
        self.as_ref().is_none_or(Blank::is_blank)
    }
}
