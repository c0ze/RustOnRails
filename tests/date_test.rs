mod support;

use std::sync::LazyLock;

use chrono::{Local, NaiveDate, Utc};
use rustonrails::{AsJson, Behavior, Ctx, Date, FromValue, Model, Record, Value, local_today, model, now, today, value_json};

model! {
    pub struct Due in "dues" { id: i64, title: String, due_on: Date }
}

impl Model for Due {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Due>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

/// The blog schema has no date column; this table lives for one test.
fn ctx() -> Ctx {
    let mut ctx = support::ctx();
    ctx.execute("CREATE TEMP TABLE dues (id bigserial PRIMARY KEY, title varchar, due_on date)", &[]).unwrap();
    ctx
}

fn date(y: i32, m: u32, d: u32) -> Date {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn cast(value: impl Into<Value>) -> Option<Date> {
    Date::from_value(value.into()).unwrap()
}

#[test]
fn test_strings_cast_like_active_model() {
    assert_eq!(Some(date(2026, 9, 25)), cast("2026-09-25"));
    assert_eq!(Some(date(2026, 9, 25)), cast(" 2026-09-25 "));
    // Date._parse keeps the date of a timestamp and ignores its offset.
    assert_eq!(Some(date(2026, 9, 25)), cast("2026-09-25T23:30:00-05:00"));
    for timestamp in ["2026-09-25 10:00:00 UTC", "2026-09-25t10:00:00.123z", "2026-09-25T10:00:00.123456+0900", "2026-09-25T"] {
        assert_eq!(Some(date(2026, 9, 25)), cast(timestamp), "{timestamp:?}");
    }
    assert_eq!(Some(date(2026, 9, 5)), cast("2026-9-5"));
    assert_eq!(Some(date(0, 1, 1)), cast("0000-01-01"));
    for nil in ["", "   ", "abc", "2026-02-30", "2026-13-01", "0000-00-00"] {
        assert_eq!(None, cast(nil), "{nil:?}");
    }
}

/// Rails hands anything else to `Date._parse`, which reads "2026/09/25"
/// and drops "2026-09-25junk"; and before 1583 its calendar is Julian.
#[test]
fn test_strings_rails_would_parse_differently_are_errors() {
    for unknown in ["2026/09/25", "Sep 25 2026", "2026-09-25junk", "1500-02-29", "1582-10-10"] {
        assert!(Date::from_value(Value::from(unknown)).is_err(), "{unknown:?}");
    }
}

#[test]
fn test_other_values_cast() {
    assert_eq!(None, cast(Value::Nil));
    assert_eq!(Some(date(2026, 9, 25)), cast(Value::Date(date(2026, 9, 25))));
    let evening = date(2026, 9, 25).and_hms_opt(23, 30, 0).unwrap();
    assert_eq!(Some(date(2026, 9, 25)), cast(Value::Time(evening)));
    assert!(Date::from_value(Value::Int(5)).is_err());
}

#[test]
fn test_renders_as_rails_does() {
    assert_eq!("2026-09-25", value_json(Value::Date(date(2026, 9, 25))));
    assert_eq!("0999-01-02", value_json(Value::Date(date(999, 1, 2))));
    assert_eq!("10000-01-01", value_json(Value::Date(date(10000, 1, 1))));
    assert_eq!("-0001-01-01", value_json(Value::Date(date(-1, 1, 1))));
    assert_eq!("2026-09-25", Value::Date(date(2026, 9, 25)).to_ruby_string());
}

#[test]
fn test_date_columns_round_trip() {
    let mut ctx = ctx();
    let due = Due::create_bang(&mut ctx, Due { title: Some("t".into()), due_on: Some(date(2026, 9, 25)), ..Due::new_record() }).unwrap();
    ctx.reload(due).unwrap();
    assert_eq!(Some(date(2026, 9, 25)), ctx[due].due_on);
    let json = AsJson::<Due>::new().only(&["due_on"]).render(&mut ctx, due).unwrap();
    assert_eq!(r#"{"due_on":"2026-09-25"}"#, json.to_string());
}

#[test]
fn test_date_columns_in_queries() {
    let mut ctx = ctx();
    Due::create_bang(&mut ctx, Due { due_on: Some(date(2026, 9, 25)), ..Due::new_record() }).unwrap();
    Due::create_bang(&mut ctx, Due { due_on: Some(date(2026, 9, 27)), ..Due::new_record() }).unwrap();
    assert_eq!(1, Due::all().where_eq("due_on", date(2026, 9, 25)).count(&mut ctx).unwrap());
    // A query value is cast by the column's type, a param string included.
    assert_eq!(1, Due::all().where_eq("due_on", "2026-09-27").count(&mut ctx).unwrap());
    assert_eq!(1, Due::all().where_gte("due_on", date(2026, 9, 26)).count(&mut ctx).unwrap());
    let assigned = Due::from_attributes(&[("due_on".into(), Value::from("2026-09-27"))]).unwrap();
    assert_eq!(Some(date(2026, 9, 27)), assigned.due_on);
}

/// `Date.current` is today in UTC, the zone Rails' config defaults to and
/// `now` assumes; `Date.today` is the machine's local date.
#[test]
fn test_today() {
    let before = now().date();
    let current = today();
    assert!(current == before || current == Utc::now().date_naive());
    let local = Local::now().date_naive();
    assert!(local_today() == local || local_today() == Local::now().date_naive());
}

/// A query value in a date format Rails parses and this doesn't
/// ("2026/09/25") fails at the bind instead of matching the NULL rows.
#[test]
fn test_a_query_date_this_cannot_read_fails() {
    let mut ctx = ctx();
    ctx.execute("INSERT INTO dues (title, due_on) VALUES ('none', NULL), ('due', '2026-09-25')", &[]).unwrap();
    assert!(Due::all().where_eq("due_on", "2026/09/25").load(&mut ctx).is_err());
    assert_eq!(1, Due::all().where_eq("due_on", "2026-09-25").load(&mut ctx).unwrap().len());
    assert_eq!(1, Due::all().where_eq("due_on", Value::Nil).load(&mut ctx).unwrap().len());
}

/// `where(created_at: Date.current..)`: a Date against a datetime column is
/// its midnight, as Active Model casts it.
#[test]
fn test_a_date_casts_to_midnight_for_a_datetime() {
    let midnight = date(2026, 9, 25).and_hms_opt(0, 0, 0).unwrap();
    assert_eq!(Some(midnight), rustonrails::Time::from_value(Value::Date(date(2026, 9, 25))).unwrap());
    assert_eq!(Some(midnight), <rustonrails::Time as FromValue>::serialize(Value::Date(date(2026, 9, 25))));
}
