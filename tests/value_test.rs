use rustonrails::{FromValue, Time, Value, now};

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

#[test]
fn test_integer_casting_matches_active_model() {
    assert_eq!(Some(42), i64::from_value(Value::from("42")).unwrap());
    assert_eq!(Some(12), i64::from_value(Value::from(" 12abc")).unwrap());
    assert_eq!(None, i64::from_value(Value::from("abc")).unwrap());
    assert_eq!(None, i64::from_value(Value::from("")).unwrap());
    assert_eq!(Some(1), i64::from_value(Value::Bool(true)).unwrap());
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
