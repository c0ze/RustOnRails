mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Check, Model, RubyString, Time, Value, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Person {
    // normalizes :email, with: ->(email) { email.strip.downcase }
    fn normalize_email(email: String) -> String {
        email.strip().downcase()
    }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::<Person>::new()
                .normalizes("email", Person::normalize_email)
                .validates("email", Check::Uniqueness { scope: &[] })
        });
        &BEHAVIOR
    }
}

fn person(email: impl Into<Value>) -> Person {
    Person::from_attributes(&[("name".into(), Value::from("ann")), ("email".into(), email.into())]).unwrap()
}

/// Assignment casts, then normalizes; nil stays nil and isn't passed in.
#[test]
fn test_assignment_normalizes_the_cast_value() {
    assert_eq!(Some("ann@example.com"), person(" Ann@Example.COM\n").email.as_deref());
    assert_eq!(None, person(Value::Nil).email);
    // A number is cast to its String first, as Active Model's string type does.
    assert_eq!(Some("42"), person(42).email.as_deref());
    // Other attributes are untouched.
    let untouched = Person::from_attributes(&[("name".into(), Value::from(" Ann "))]).unwrap();
    assert_eq!(Some(" Ann "), untouched.name.as_deref());
}

/// `where` and `find_by` normalize what they're given, so a record saved
/// from one spelling is found by another, and uniqueness sees them as one.
#[test]
fn test_queries_normalize_their_values() {
    let mut ctx = support::ctx();
    let ann = ctx.build(person("ann@example.com"));
    ctx.save_bang(ann).unwrap();
    let found = Person::find_by(&mut ctx, "email", " ANN@example.com ").unwrap();
    assert_eq!(ctx[ann].id, found.and_then(|f| ctx[f].id));
    assert_eq!(1, Person::all().where_eq("email", "Ann@Example.com").count(&mut ctx).unwrap());
    assert_eq!(0, Person::all().where_not("email", "ANN@EXAMPLE.COM").where_eq("id", ctx[ann].id).count(&mut ctx).unwrap());
    assert_eq!(1, Person::all().where_in("email", vec!["x".into(), " ann@EXAMPLE.com".into()]).count(&mut ctx).unwrap());
    assert!(Person::all().where_eq("email", Value::Nil).to_sql().0.contains(r#""users"."email" IS NULL"#));

    let twin = ctx.build(person("ANN@example.com "));
    assert!(!ctx.save(twin).unwrap());
    assert_eq!(vec!["has already been taken"], ctx.errors(twin).on("email"));
}
