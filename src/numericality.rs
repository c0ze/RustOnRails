//! `validates ..., numericality:` as Active Model 8.1 checks it: the value
//! as it was given, read the way `NumericalityValidator` reads it, and
//! compared exactly, as Ruby's Integer and BigDecimal compare.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use crate::Value;

/// A numericality option: an Integer or a Float in the Ruby.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Float(f64),
}

/// `numericality:`'s options. Every comparison that fails adds its
/// message, in Rails' order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Numericality {
    pub only_integer: bool,
    pub greater_than: Option<Number>,
    pub greater_than_or_equal_to: Option<Number>,
    pub equal_to: Option<Number>,
    pub less_than: Option<Number>,
    pub less_than_or_equal_to: Option<Number>,
    pub other_than: Option<Number>,
}

impl Numericality {
    /// The messages for `value`, the attribute as it was given.
    pub(crate) fn messages(&self, value: &Value) -> Vec<String> {
        let Some(number) = parse(value) else { return vec!["is not a number".into()] };
        if self.only_integer && !is_integer(value) {
            return vec!["must be an integer".into()];
        }
        use Ordering::{Equal, Greater, Less};
        // None is NaN, which compares false; `!=` is `!(==)` in Ruby.
        type Test = fn(Option<Ordering>) -> bool;
        let checks: [(Option<Number>, &str, Test); 6] = [
            (self.greater_than, "greater than", |o| o == Some(Greater)),
            (self.greater_than_or_equal_to, "greater than or equal to", |o| matches!(o, Some(Greater | Equal))),
            (self.equal_to, "equal to", |o| o == Some(Equal)),
            (self.less_than, "less than", |o| o == Some(Less)),
            (self.less_than_or_equal_to, "less than or equal to", |o| matches!(o, Some(Less | Equal))),
            (self.other_than, "other than", |o| o != Some(Equal)),
        ];
        checks
            .into_iter()
            .filter_map(|(option, phrase, passes)| {
                let option = option?;
                let bound = Exact::from(option);
                (!passes(number.compare(&bound))).then(|| format!("must be {phrase} {}", count(option, &bound)))
            })
            .collect()
    }
}

/// `%{count}`: an Integer as is, a Float as the BigDecimal Rails compared,
/// which ActiveSupport prints as "1.0".
fn count(option: Number, bound: &Exact) -> String {
    match option {
        Number::Int(i) => i.to_string(),
        Number::Float(_) => bound.to_plain(),
    }
}

/// Ruby's `\A[+-]?\d+\z`; `\d` is ASCII in Ruby, Unicode in the regex crate.
static INTEGER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?[0-9]+$").expect("regex"));
static HEXADECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?0[xX]").expect("regex"));
/// Digits with single underscores between them, as Ruby's `Float()` takes.
const DIGITS: &str = "[0-9](?:_?[0-9])*";
const HEX_DIGITS: &str = "[0-9a-fA-F](?:_?[0-9a-fA-F])*";
static DECIMAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?:{DIGITS}(?:\.(?:{DIGITS})?)?|\.{DIGITS})(?:[eE][+-]?{DIGITS})?$")).expect("regex"));
static HEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^((?:{HEX_DIGITS})?)(?:\.((?:{HEX_DIGITS})?))?(?:[pP]([+-]?{DIGITS}))?$")).expect("regex")
});

/// `to_s` matches the integer pattern: Integers do, Floats never do.
fn is_integer(value: &Value) -> bool {
    match value {
        Value::Int(_) => true,
        Value::Str(s) => INTEGER.is_match(s),
        _ => false,
    }
}

/// `parse_as_number`: Integers and integer strings exactly, Floats and
/// other strings through `Kernel#Float` rounded to 15 digits, and nothing
/// for hex literals or values `Float()` raises on (nil, booleans, dates).
fn parse(value: &Value) -> Option<Exact> {
    match value {
        Value::Int(i) => Some(Exact::integer(&i.to_string())),
        Value::Float(f) => Some(Exact::float(*f)),
        Value::Str(s) if INTEGER.is_match(s) => Some(Exact::integer(s)),
        Value::Str(s) if HEXADECIMAL.is_match(s) => None,
        Value::Str(s) => ruby_float(s).map(Exact::float),
        _ => None,
    }
}

/// Ruby's `Kernel#Float` on a string, None where it raises: ASCII
/// whitespace around an optional sign and a decimal or hex literal. The
/// hex check above has no leading whitespace, so " 0x1A" gets here.
fn ruby_float(s: &str) -> Option<f64> {
    let s = s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let magnitude = match body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        Some(hex) => hex_float(hex)?,
        None if DECIMAL.is_match(body) => body.replace('_', "").parse().ok()?,
        None => return None,
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// A hex literal after `0x`: digits, a fraction and a binary exponent,
/// any of which may be missing as long as something is there.
fn hex_float(hex: &str) -> Option<f64> {
    if hex.is_empty() {
        return None;
    }
    let parts = HEX.captures(hex)?;
    let (int, frac) = (parts[1].replace('_', ""), parts.get(2).map_or(String::new(), |m| m.as_str().replace('_', "")));
    if int.is_empty() && frac.is_empty() && !hex.starts_with('.') {
        return None;
    }
    // An exponent past an i64 is still just very large or very small:
    // Ruby reads 0x1p-99999999999999999999 as 0.0.
    let exponent = parts.get(3).map_or(0, |m| {
        let text = m.as_str().replace('_', "");
        text.parse().unwrap_or(if text.starts_with('-') { i64::MIN } else { i64::MAX })
    });
    // 32 hex digits fill a u128; any beyond only decide rounding, so they
    // become one sticky bit.
    let digits = format!("{int}{frac}");
    let digits = digits.trim_start_matches('0');
    let kept = &digits[..digits.len().min(32)];
    let mut mantissa = u128::from_str_radix(if kept.is_empty() { "0" } else { kept }, 16).ok()?;
    if digits[kept.len()..].chars().any(|c| c != '0') {
        mantissa |= 1;
    }
    let shift = exponent.saturating_sub(4 * frac.len() as i64).saturating_add(4 * (digits.len() - kept.len()) as i64);
    Some(scale(mantissa as f64, shift))
}

/// `x * 2^shift` in steps that can't overflow on the way.
fn scale(mut x: f64, mut shift: i64) -> f64 {
    while shift != 0 && x != 0.0 && x.is_finite() {
        let step = shift.clamp(-1000, 1000);
        x *= 2f64.powi(step as i32);
        shift -= step;
    }
    x
}

/// A number as Rails compares it: an Integer, or a BigDecimal holding a
/// Float rounded to 15 significant digits. A finite one is
/// 0.d1d2d3... × 10^point with no leading or trailing zero digits; zero
/// has none.
#[derive(Clone, Debug, PartialEq)]
enum Exact {
    Finite { negative: bool, digits: Vec<u8>, point: i64 },
    Infinite { negative: bool },
    NaN,
}

impl From<Number> for Exact {
    fn from(number: Number) -> Self {
        match number {
            Number::Int(i) => Exact::integer(&i.to_string()),
            Number::Float(f) => Exact::float(f),
        }
    }
}

impl Exact {
    fn finite(negative: bool, digits: Vec<u8>, point: i64) -> Self {
        let leading = digits.iter().take_while(|d| **d == 0).count();
        let mut digits = digits[leading..].to_vec();
        while digits.last() == Some(&0) {
            digits.pop();
        }
        Exact::Finite { negative: negative && !digits.is_empty(), point: if digits.is_empty() { 0 } else { point - leading as i64 }, digits }
    }

    /// An integer's digits, as `to_i` reads them: any length, like a Bignum.
    fn integer(s: &str) -> Self {
        let digits: Vec<u8> = s.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
        let point = digits.len() as i64;
        Exact::finite(s.starts_with('-'), digits, point)
    }

    /// `Float#to_d(15)`: dtoa's 15 significant digits, rounded to nearest
    /// with ties to even, which is also how Rust formats.
    fn float(f: f64) -> Self {
        if f.is_nan() {
            return Exact::NaN;
        }
        if f.is_infinite() {
            return Exact::Infinite { negative: f < 0.0 };
        }
        let formatted = format!("{:.14e}", f.abs());
        let (mantissa, exponent) = formatted.split_once('e').expect("{:e} has an exponent");
        let digits = mantissa.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
        Exact::finite(f < 0.0, digits, exponent.parse::<i64>().expect("integer exponent") + 1)
    }

    fn signum(&self) -> i8 {
        match self {
            Exact::Finite { digits, .. } if digits.is_empty() => 0,
            Exact::Finite { negative, .. } | Exact::Infinite { negative } => if *negative { -1 } else { 1 },
            Exact::NaN => 0,
        }
    }

    /// None when either side is NaN, which compares false with anything.
    fn compare(&self, other: &Exact) -> Option<Ordering> {
        if matches!(self, Exact::NaN) || matches!(other, Exact::NaN) {
            return None;
        }
        // -Infinity, negatives, zero, positives, Infinity; then magnitude.
        let rank = |e: &Exact| match e {
            Exact::Infinite { negative: true } => -2,
            Exact::Infinite { .. } => 2,
            _ => e.signum(),
        };
        match (self, other) {
            (Exact::Finite { digits: a, point: pa, .. }, Exact::Finite { digits: b, point: pb, .. }) if rank(self) == rank(other) => {
                let magnitude = pa.cmp(pb).then_with(|| a.cmp(b));
                Some(if self.signum() < 0 { magnitude.reverse() } else { magnitude })
            }
            _ => Some(rank(self).cmp(&rank(other))),
        }
    }

    /// BigDecimal's `to_s("F")`: plain digits, at least one after the point.
    fn to_plain(&self) -> String {
        let (negative, digits, point) = match self {
            Exact::Finite { negative, digits, point } => (*negative, digits, *point),
            Exact::Infinite { negative } => return if *negative { "-Infinity" } else { "Infinity" }.into(),
            Exact::NaN => return "NaN".into(),
        };
        let digits: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
        let (int, frac) = if digits.is_empty() {
            ("0".to_string(), String::new())
        } else if point <= 0 {
            ("0".to_string(), format!("{}{digits}", "0".repeat(point.unsigned_abs() as usize)))
        } else if point as usize >= digits.len() {
            (format!("{digits}{}", "0".repeat(point as usize - digits.len())), String::new())
        } else {
            (digits[..point as usize].to_string(), digits[point as usize..].to_string())
        };
        format!("{}{int}.{}", if negative { "-" } else { "" }, if frac.is_empty() { "0" } else { &frac })
    }
}
