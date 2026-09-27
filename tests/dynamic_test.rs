//! Ruby's operators on values whose class is known only at run time. Each
//! expectation is what Ruby 3.4 answers or raises.

use rustonrails::{Error, Value};

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn float(f: f64) -> Value {
    Value::Float(f)
}

fn s(text: &str) -> Value {
    Value::from(text)
}

fn message(result: rustonrails::Result<impl std::fmt::Debug>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn test_addition() {
    assert_eq!(int(3), int(1).add(&int(2)).unwrap());
    assert_eq!(float(2.5), int(1).add(&float(1.5)).unwrap());
    assert_eq!(float(2.5), float(1.5).add(&int(1)).unwrap());
    assert_eq!(s("ab"), s("a").add(&s("b")).unwrap());
    assert_eq!("String can't be coerced into Integer", message(int(1).add(&s("a"))));
    assert_eq!("nil can't be coerced into Integer", message(int(1).add(&Value::Nil)));
    assert_eq!("true can't be coerced into Integer", message(int(1).add(&Value::Bool(true))));
    assert_eq!("String can't be coerced into Float", message(float(1.5).add(&s("a"))));
    assert_eq!("no implicit conversion of Integer into String", message(s("a").add(&int(1))));
    assert_eq!("no implicit conversion of nil into String", message(s("a").add(&Value::Nil)));
    assert!(matches!(Value::Nil.add(&int(1)), Err(Error::Nil { what: "+" })));
    assert!(matches!(Value::Bool(true).add(&int(1)), Err(Error::NoMethod { what: "+", .. })));
    assert!(matches!(int(i64::MAX).add(&int(1)), Err(Error::Overflow { .. })));
}

#[test]
fn test_subtraction_and_multiplication() {
    assert_eq!(int(-1), int(1).sub(&int(2)).unwrap());
    assert!(matches!(s("a").sub(&s("b")), Err(Error::NoMethod { what: "-", .. })));
    assert_eq!(int(6), int(2).mul(&int(3)).unwrap());
    assert_eq!(s("abab"), s("ab").mul(&int(2)).unwrap());
    assert_eq!(s("abab"), s("ab").mul(&float(2.5)).unwrap());
    assert_eq!("negative argument", message(s("ab").mul(&int(-1))));
    assert_eq!("no implicit conversion of String into Integer", message(s("ab").mul(&s("c"))));
    assert_eq!("String can't be coerced into Integer", message(int(2).mul(&s("ab"))));
    assert_eq!("argument too big", message(s("ab").mul(&int(1 << 40))));
}

#[test]
fn test_time_and_date_arithmetic() {
    let t = rustonrails::now();
    let later = Value::Time(t).add(&int(90)).unwrap();
    assert_eq!(float(90.0), later.sub(&Value::Time(t)).unwrap());
    assert_eq!(Value::Time(t), later.sub(&int(90)).unwrap());
    let d = rustonrails::today();
    assert_eq!(Value::Date(d + chrono::Duration::days(3)), Value::Date(d).add(&int(3)).unwrap());
}

#[test]
fn test_comparisons() {
    assert!(int(1).compare("<", &float(1.5)).unwrap());
    assert!(!int(1).compare("<", &float(f64::NAN)).unwrap());
    assert!(!float(f64::NAN).compare(">=", &float(f64::NAN)).unwrap());
    assert!(s("a").compare("<", &s("b")).unwrap());
    assert!(int(2).compare(">=", &int(2)).unwrap());
    // Exactly, not through a Float: 2^53 + 1 is past 2^53 as a Float.
    assert!(int(9_007_199_254_740_993).compare(">", &float(9_007_199_254_740_992.0)).unwrap());
    assert!(!int(9_007_199_254_740_993).equals(&float(9_007_199_254_740_992.0)));
    assert_eq!("comparison of Integer with String failed", message(int(1).compare("<", &s("a"))));
    assert_eq!("comparison of Integer with nil failed", message(int(1).compare("<", &Value::Nil)));
    assert_eq!("comparison of String with 1 failed", message(s("a").compare("<", &int(1))));
    assert!(matches!(Value::Nil.compare("<", &int(1)), Err(Error::Nil { what: "<" })));
    assert!(matches!(Value::Bool(true).compare("<", &int(1)), Err(Error::NoMethod { what: "<", .. })));
}

#[test]
fn test_equality_and_truth() {
    assert!(int(1).equals(&float(1.0)));
    assert!(!s("1").equals(&int(1)));
    assert!(Value::Nil.equals(&Value::Nil));
    assert!(!float(f64::NAN).equals(&float(f64::NAN)));
    assert!(int(0).is_truthy());
    assert!(s("").is_truthy());
    assert!(!Value::Nil.is_truthy());
    assert!(!Value::Bool(false).is_truthy());
}

#[test]
fn test_conversions() {
    assert_eq!("", Value::Nil.to_s());
    assert_eq!("1.0", float(1.0).to_s());
    assert_eq!("1.0e+20", float(1e20).to_s());
    assert_eq!("true", Value::Bool(true).to_s());
    let t = chrono::NaiveDate::from_ymd_opt(2026, 9, 26).unwrap().and_hms_opt(1, 2, 3).unwrap();
    assert_eq!("2026-09-26 01:02:03 UTC", Value::Time(t).to_s());
    assert_eq!(0.0, Value::Nil.to_f().unwrap());
    assert_eq!(12.5, s("12.5kg").to_f().unwrap());
    assert!(matches!(Value::Bool(true).to_f(), Err(Error::NoMethod { what: "to_f", .. })));
}

#[test]
fn test_division_and_modulo() {
    use rustonrails::{div_integers, mod_floats, mod_integers};
    assert_eq!(3, div_integers(7, 2).unwrap());
    assert_eq!(-4, div_integers(-7, 2).unwrap());
    assert_eq!(-4, div_integers(7, -2).unwrap());
    assert_eq!(-1, mod_integers(7, -2).unwrap());
    assert_eq!(1, mod_integers(-7, 2).unwrap());
    assert_eq!(0, mod_integers(i64::MIN, -1).unwrap());
    assert!(matches!(div_integers(7, 0), Err(Error::ZeroDivision)));
    assert!(matches!(mod_integers(7, 0), Err(Error::ZeroDivision)));
    assert!(matches!(div_integers(i64::MIN, -1), Err(Error::Overflow { .. })));
    assert_eq!(0.5, mod_floats(-7.5, 2.0).unwrap());
    assert_eq!(-0.5, mod_floats(7.5, -2.0).unwrap());
    assert!(mod_floats(f64::INFINITY, 2.0).unwrap().is_nan());
    assert_eq!(2.0, mod_floats(2.0, f64::INFINITY).unwrap());
    assert_eq!(f64::INFINITY, mod_floats(-2.0, f64::INFINITY).unwrap());
    assert!(mod_floats(-6.0, 3.0).unwrap().is_sign_negative());
    assert!(matches!(mod_floats(7.0, 0.0), Err(Error::ZeroDivision)));

    assert_eq!(int(3), int(7).div(&int(2)).unwrap());
    assert_eq!(float(3.5), int(7).div(&float(2.0)).unwrap());
    assert_eq!(float(f64::INFINITY), float(7.0).div(&int(0)).unwrap());
    assert!(matches!(int(7).div(&int(0)), Err(Error::ZeroDivision)));
    assert!(matches!(int(7).modulo(&float(0.0)), Err(Error::ZeroDivision)));
    assert_eq!(float(0.5), float(-7.5).modulo(&int(2)).unwrap());
    assert!(matches!(s("a").div(&int(2)), Err(Error::NoMethod { what: "/", .. })));
    assert!(matches!(Value::Nil.div(&int(2)), Err(Error::Nil { what: "/" })));
    assert_eq!("String can't be coerced into Integer", message(int(7).div(&s("a"))));
    assert_eq!("nil can't be coerced into Integer", message(int(7).modulo(&Value::Nil)));
}

/// Active Support compares a Time with a Date as the Date's midnight.
#[test]
fn test_time_against_date() {
    let d = chrono::NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
    let midnight = Value::Time(d.and_hms_opt(0, 0, 0).unwrap());
    let noon = Value::Time(d.and_hms_opt(12, 0, 0).unwrap());
    assert!(noon.compare(">", &Value::Date(d)).unwrap());
    assert!(Value::Date(d).compare("<", &noon).unwrap());
    assert!(!midnight.compare(">", &Value::Date(d)).unwrap());
    assert!(midnight.equals(&Value::Date(d)) && Value::Date(d).equals(&midnight));
    assert!(!noon.equals(&Value::Date(d)));
}

/// Where Ruby's Date and Time go further than chrono's, an error, not a panic.
#[test]
fn test_dates_and_times_out_of_range() {
    let d = Value::Date(rustonrails::today());
    assert_eq!("time out of range", message(d.add(&int(100_000_000))));
    assert_eq!("time out of range", message(d.sub(&int(i64::MAX))));
    let t = Value::Time(rustonrails::now());
    assert_eq!("time out of range", message(t.add(&float(9e12))));
    assert_eq!("time out of range", message(t.add(&int(i64::MAX / 1_000_000))));
}

#[test]
fn test_repeating_a_string_by_a_float_that_isnt_a_number() {
    assert_eq!("NaN", message(s("ab").mul(&float(f64::NAN))));
    assert_eq!("Infinity", message(s("ab").mul(&float(f64::INFINITY))));
    assert_eq!(s("abab"), s("ab").mul(&float(2.5)).unwrap());
}

#[test]
fn test_negative_zero_keeps_its_sign() {
    assert_eq!("-0.0", float(-0.0).to_s());
    assert_eq!("0.0", float(0.0).to_s());
}
