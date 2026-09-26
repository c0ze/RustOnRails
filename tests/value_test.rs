use rustonrails::{Error, FromValue, Time, Value, now};

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

// Checked against ActiveModel::Type::Integer#cast in Rails 8.1.4.
#[test]
fn test_integer_casting_matches_active_model() {
    assert_eq!(Some(42), i64::from_value(Value::from("42")).unwrap());
    assert_eq!(Some(12), i64::from_value(Value::from(" 12abc")).unwrap());
    assert_eq!(Some(1), i64::from_value(Value::from("1.9")).unwrap());
    assert_eq!(Some(0), i64::from_value(Value::from("abc")).unwrap());
    assert_eq!(None, i64::from_value(Value::from("")).unwrap());
    assert_eq!(Some(1), i64::from_value(Value::Bool(true)).unwrap());
}

// ActiveModel::Type::Float#cast
#[test]
fn test_float_casting_matches_active_model() {
    assert_eq!(Some(1.5), f64::from_value(Value::from("1.5x")).unwrap());
    assert_eq!(Some(0.0), f64::from_value(Value::from("abc")).unwrap());
    assert_eq!(None, f64::from_value(Value::from("")).unwrap());
}

// ActiveModel::Type::String#cast and Float#to_s
#[test]
fn test_floats_print_like_ruby() {
    assert_eq!(Some("1.0".to_string()), String::from_value(Value::Float(1.0)).unwrap());
    assert_eq!(Some("1.5".to_string()), String::from_value(Value::Float(1.5)).unwrap());
    assert_eq!(Some("1.0e+20".to_string()), String::from_value(Value::Float(1e20)).unwrap());
}

// ActiveModel::Type::DateTime#cast: offsets become UTC, dates become midnight.
#[test]
fn test_time_casting_handles_offsets_and_dates() {
    let parse = |s: &str| Time::from_value(Value::from(s)).unwrap().map(|t| t.to_string());
    assert_eq!(Some("2026-09-25 03:00:00".to_string()), parse("2026-09-25T12:00:00+09:00"));
    assert_eq!(Some("2026-09-25 00:00:00".to_string()), parse("2026-09-25"));
    assert_eq!(Some("2026-09-25 12:00:00.123456".to_string()), parse("2026-09-25T12:00:00.123456Z"));
    assert_eq!(None, parse("junk"));
    assert_eq!(None, parse(" "));
    // An HTML datetime-local value, the same with a space, and Time#to_s,
    // each as Rails 8.1 casts them.
    assert_eq!(Some("2026-09-26 10:00:00".to_string()), parse("2026-09-26T10:00"));
    assert_eq!(Some("2026-09-26 10:00:00".to_string()), parse("2026-09-26 10:00"));
    assert_eq!(Some("2026-09-26 10:00:00".to_string()), parse("2026-09-26 10:00:00 UTC"));
    assert_eq!(Some("2026-09-26 01:00:00".to_string()), parse("2026-09-26T10:00+09:00"));
    // Date._parse reads these; a format this doesn't know is an error, not
    // a nil that saves NULL.
    for unknown in ["Sep 26 2026 10:00", "26/09/2026", "2026-09-26 junk"] {
        assert!(Time::from_value(Value::from(unknown)).is_err(), "{unknown:?}");
    }
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

// Checked against Ruby 3.4's String#to_i, Float#to_i and nil.to_i.
#[test]
fn test_to_i_follows_ruby() {
    let cases = [
        ("42abc", 42), ("  -12", -12), ("+5", 5), ("1_000", 1000), ("1__0", 1), ("_1", 0), ("abc", 0), ("", 0),
        ("012", 12), ("0x1A", 0), ("-_1", 0), ("1_", 1), ("\t\n7", 7), ("2abc", 2), ("0", 0),
    ];
    for (text, expected) in cases {
        assert_eq!(expected, Value::from(text).to_i().unwrap(), "{text:?}");
    }
    assert_eq!(0, Value::Nil.to_i().unwrap());
    assert_eq!(7, Value::Int(7).to_i().unwrap());
    assert_eq!(2, Value::Float(2.9).to_i().unwrap());
    assert_eq!(-2, Value::Float(-2.9).to_i().unwrap());
    let time: Time = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap().naive_utc();
    assert_eq!(1_700_000_000, Value::Time(time).to_i().unwrap());
}

/// Where Ruby would make a Bignum, or has no to_i, this fails.
#[test]
fn test_to_i_fails_where_ruby_cant_fit_an_i64() {
    assert_eq!(i64::MAX, Value::from("9223372036854775807").to_i().unwrap());
    assert!(matches!(Value::from("99999999999999999999").to_i(), Err(Error::Overflow { .. })));
    assert!(matches!(Value::Float(1e20).to_i(), Err(Error::Overflow { .. })));
    assert!(matches!(Value::Float(f64::NAN).to_i(), Err(Error::Overflow { .. })));
    assert!(matches!(Value::Bool(true).to_i(), Err(Error::NoMethod { what: "to_i", .. })));
}

/// Only a String answers to_str; nil fails as Ruby's NoMethodError on nil does.
#[test]
fn test_to_str_only_for_strings() {
    assert_eq!("q", Value::from("q").to_str().unwrap());
    assert!(matches!(Value::Int(5).to_str(), Err(Error::NoMethod { what: "to_str", .. })));
    assert!(matches!(Value::Nil.to_str(), Err(Error::Nil { what: "to_str" })));
}

/// Ruby promotes an overflowing Integer to a Bignum; generated code must
/// fail instead of wrapping, in release builds too.
#[test]
fn test_release_builds_check_overflow() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("[profile.release]\noverflow-checks = true"), "{manifest}");
}
