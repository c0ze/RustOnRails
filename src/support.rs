//! Core Ruby and ActiveSupport behavior that generated code calls where
//! Rust's nearest method differs from Ruby's.

use serde_json::Value as Json;

use crate::{Error, Result, Value};

/// `Array#sum` of Integers, from `start`. Where Ruby would go on in a
/// Bignum this fails, naming the sum.
pub fn sum_integers(start: i64, values: impl IntoIterator<Item = i64>) -> Result<i64> {
    let sum = values.into_iter().fold(i128::from(start), |sum, value| sum + i128::from(value));
    i64::try_from(sum).map_err(|_| Error::Overflow { value: sum.to_string() })
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
