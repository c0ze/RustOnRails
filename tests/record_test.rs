mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Model, Record, Time, Value, model};

model! {
    pub struct Note in "posts" {
        id: i64,
        title: String,
        status: String = "draft",
        comments_count: i64 = 0,
        published_at: Time,
    }
}

impl Model for Note {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Note>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

#[test]
fn test_model_macro_describes_the_table() {
    assert_eq!("Note", Note::NAME);
    assert_eq!("posts", Note::TABLE);
    assert_eq!(&["id", "title", "status", "comments_count", "published_at"], Note::COLUMNS);
}

#[test]
fn test_new_record_takes_column_defaults() {
    let note = Note::new_record();
    assert_eq!(Some("draft".to_string()), note.status);
    assert_eq!(Some(0), note.comments_count);
    assert_eq!(None, note.title);
}

#[test]
fn test_attributes_by_name() {
    let mut note = Note::new_record();
    note.set("title", Value::from("Hi")).unwrap();
    assert_eq!(Value::from("Hi"), note.get("title"));
    assert_eq!(Value::Nil, note.get("published_at"));
    assert!(note.set("nope", Value::Nil).is_err());
}

#[test]
fn test_integer_attributes_cast_like_active_model() {
    let mut note = Note::new_record();
    note.set("comments_count", Value::from("")).unwrap();
    assert_eq!(None, note.comments_count);
    note.set("comments_count", Value::from("3")).unwrap();
    assert_eq!(Some(3), note.comments_count);
    note.set("comments_count", Value::from("1_000")).unwrap();
    assert_eq!(Some(1000), note.comments_count);
}

/// `where(comments_count: "99999999999999999999")` fails at the bind
/// rather than matching the rows holding what assignment casts it to.
#[test]
fn test_an_integer_too_big_to_query_keeps_its_value() {
    assert_eq!(Value::Int(7), Note::cast_query("comments_count", Value::from("7")));
    assert_eq!(Value::Nil, Note::cast_query("comments_count", Value::from("abc")));
    let big = Value::from("99999999999999999999");
    assert_eq!(big, Note::cast_query("comments_count", big.clone()));
}

#[test]
fn test_handles_alias_like_ruby_variables() {
    let mut ctx = support::ctx();
    let a = ctx.build(Note::new_record());
    let b = a;
    ctx[b].title = Some("x".into());
    assert_eq!(Some("x"), ctx[a].title.as_deref());
    assert!(ctx.is_new_record(a));
    assert!(!ctx.is_persisted(a));
}

#[test]
fn test_changed_compares_with_defaults_for_new_records() {
    let mut ctx = support::ctx();
    let note = ctx.build(Note::new_record());
    assert!(ctx.changed(note).is_empty());
    ctx[note].title = Some("x".into());
    assert_eq!(vec!["title"], ctx.changed(note));
    assert!(ctx.attribute_changed(note, "title"));
}

/// Active Model's numeric types count a number replaced by a string that
/// doesn't start like one as a change, though both cast to 0.
#[test]
fn test_a_non_numeric_string_over_a_number_is_a_change() {
    let mut ctx = support::ctx();
    let note = ctx.build(Note::new_record());
    ctx.assign(note, &[("comments_count".into(), Value::from("abc"))]).unwrap();
    assert_eq!(Some(0), ctx[note].comments_count);
    assert!(ctx.attribute_changed(note, "comments_count"));
    for same in [Value::from(" 0"), Value::from("-0x"), Value::Int(0), Value::Float(0.0)] {
        ctx.assign(note, &[("comments_count".into(), same.clone())]).unwrap();
        assert!(!ctx.attribute_changed(note, "comments_count"), "{same:?}");
    }
    ctx.assign(note, &[("comments_count".into(), Value::Bool(false))]).unwrap();
    assert!(ctx.attribute_changed(note, "comments_count"));
}

#[test]
fn test_errors_keep_order_and_humanize() {
    let mut ctx = support::ctx();
    let note = ctx.build(Note::new_record());
    ctx.errors_mut(note).add("title", "can't be blank");
    ctx.errors_mut(note).add("user_id", "must exist");
    ctx.errors_mut(note).add("title", "is too short (minimum is 3 characters)");
    let errors = ctx.errors(note);
    assert_eq!(vec!["can't be blank", "is too short (minimum is 3 characters)"], errors.on("title"));
    assert_eq!(vec!["title", "user_id"], errors.attributes());
    assert_eq!("User must exist", errors.full_messages()[1]);
}
