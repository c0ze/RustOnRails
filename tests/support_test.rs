use rustonrails::{Blank, Json, RubyString, Value, json};

#[test]
fn test_strip_is_rubys() {
    assert_eq!("a", " \t\n\x0b\x0c\r\0a\0 ".strip());
    // Ruby leaves non-breaking spaces alone; Rust's trim doesn't.
    assert_eq!("\u{a0}a\u{a0}", " \u{a0}a\u{a0} ".strip());
}

#[test]
fn test_downcase_has_no_final_sigma() {
    assert_eq!("σασ", "ΣΑΣ".downcase());
    assert_eq!("àb", "ÀB".downcase());
    assert_eq!("SS", "ß".upcase());
}

#[test]
fn test_blank_and_present() {
    assert!(" \u{3000}\n".is_blank());
    assert!(None::<&Json>.is_blank());
    assert!(Some(&json!("")).is_blank());
    assert!(Some(&json!(false)).is_blank());
    assert!(Some(&json!("a")).is_present());
    assert!(Some(&json!(["a"])).is_present());
    assert!(Some(&json!(0)).is_present());
    assert!(Value::Nil.is_blank());
    assert!(Some("x".to_string()).is_present());
}
