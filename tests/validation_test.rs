mod support;

use std::sync::LazyLock;

use regex::Regex;
use rustonrails::{Behavior, BelongsTo, Check, Ctx, Handle, Model, Record, Result, Time, Value, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::<Person>::new()
                .before_validation(|ctx, person| {
                    let email = ctx[person].email.clone().unwrap_or_default();
                    ctx[person].email = Some(email.trim().to_lowercase());
                    Ok(())
                })
                .validates("name", Check::Presence)
                .validates("name", Check::Length { minimum: Some(2), maximum: Some(5) })
                .validates("email", Check::Uniqueness { scope: &[] })
                .validates("email", Check::Format(Regex::new(r"\A[^@\s]+@[^@\s]+\z").unwrap()))
                .unless(|ctx, person| ctx[person].name.as_deref() == Some("skip"))
                .validate(no_bobs)
        });
        &BEHAVIOR
    }
}

fn no_bobs(ctx: &mut Ctx, person: Handle<Person>) -> Result<()> {
    if ctx[person].name.as_deref() == Some("Bob") {
        ctx.errors_mut(person).add("name", "is reserved");
    }
    Ok(())
}

model! {
    pub struct Entry in "posts" { id: i64, user_id: i64, title: String, status: String = "draft" }
}

const ENTRY_USER: BelongsTo<Entry, Person> = BelongsTo::new("user", "user_id");

impl Model for Entry {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Entry>> = LazyLock::new(|| {
            Behavior::<Entry>::new()
                .belongs_to(&ENTRY_USER)
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
        });
        &BEHAVIOR
    }
}

fn person(ctx: &mut Ctx, name: &str, email: &str) -> Handle<Person> {
    ctx.build(Person { name: Some(name.into()), email: Some(email.into()), ..Person::new_record() })
}

fn insert_user(ctx: &mut Ctx, email: &str) -> i64 {
    let rows = ctx
        .query(
            "INSERT INTO users (name, email, created_at, updated_at) VALUES ('Taken', $1, now(), now()) RETURNING id",
            &[Value::from(email)],
        )
        .unwrap();
    rows[0].get(0)
}

#[test]
fn test_presence_rejects_nil_and_whitespace() {
    let mut ctx = support::ctx();
    let nobody = ctx.build(Person::new_record());
    assert!(!ctx.is_valid(nobody).unwrap());
    assert!(ctx.errors(nobody).on("name").contains(&"can't be blank"));
    let spaces = person(&mut ctx, "   ", "a@b.c");
    assert!(!ctx.is_valid(spaces).unwrap());
    assert!(ctx.errors(spaces).on("name").contains(&"can't be blank"));
}

#[test]
fn test_length_messages() {
    let mut ctx = support::ctx();
    let short = person(&mut ctx, "A", "a@b.c");
    ctx.is_valid(short).unwrap();
    assert_eq!(vec!["is too short (minimum is 2 characters)"], ctx.errors(short).on("name"));
    let long = person(&mut ctx, "Abcdef", "a@b.c");
    ctx.is_valid(long).unwrap();
    assert_eq!(vec!["is too long (maximum is 5 characters)"], ctx.errors(long).on("name"));
}

#[test]
fn test_before_validation_runs_first_and_uniqueness_queries() {
    let mut ctx = support::ctx();
    insert_user(&mut ctx, "taken@example.com");
    let dup = person(&mut ctx, "Carol", "  TAKEN@example.com ");
    assert!(!ctx.is_valid(dup).unwrap());
    assert_eq!(Some("taken@example.com"), ctx[dup].email.as_deref());
    assert_eq!(vec!["has already been taken"], ctx.errors(dup).on("email"));
}

#[test]
fn test_format_and_unless() {
    let mut ctx = support::ctx();
    let bad = person(&mut ctx, "Dan", "nope");
    ctx.is_valid(bad).unwrap();
    assert_eq!(vec!["is invalid"], ctx.errors(bad).on("email"));
    let skipped = person(&mut ctx, "skip", "nope");
    assert!(ctx.is_valid(skipped).unwrap());
}

#[test]
fn test_custom_validation_and_error_reset() {
    let mut ctx = support::ctx();
    let bob = person(&mut ctx, "Bob", "bob@example.com");
    assert!(!ctx.is_valid(bob).unwrap());
    assert_eq!(vec!["is reserved"], ctx.errors(bob).on("name"));
    ctx[bob].name = Some("Rob".into());
    assert!(ctx.is_valid(bob).unwrap());
    assert!(ctx.errors(bob).is_empty());
}

#[test]
fn test_belongs_to_must_exist_and_enum_inclusion() {
    let mut ctx = support::ctx();
    let orphan = ctx.build(Entry { user_id: Some(0), title: Some("t".into()), status: Some("archived".into()), ..Entry::new_record() });
    assert!(!ctx.is_valid(orphan).unwrap());
    assert_eq!(vec!["must exist"], ctx.errors(orphan).on("user"));
    assert_eq!(vec!["is not included in the list"], ctx.errors(orphan).on("status"));
    let missing = ctx.build(Entry::new_record());
    ctx.is_valid(missing).unwrap();
    assert_eq!(vec!["must exist"], ctx.errors(missing).on("user"));
    let owner = insert_user(&mut ctx, "owner@example.com");
    let fine = ctx.build(Entry { user_id: Some(owner), ..Entry::new_record() });
    assert!(ctx.is_valid(fine).unwrap());
}

model! {
    pub struct Titled in "posts" { id: i64, user_id: i64, title: String, body: String }
}

impl Model for Titled {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Titled>> = LazyLock::new(|| {
            Behavior::<Titled>::new()
                .validates("title", Check::Uniqueness { scope: &["user_id"] })
                .validates("body", Check::Presence)
                .allow_nil()
                .validates("body", Check::Length { minimum: Some(2), maximum: None })
                .allow_blank()
        });
        &BEHAVIOR
    }
}

fn titled(ctx: &mut Ctx, user_id: Option<i64>, body: Option<&str>) -> Handle<Titled> {
    ctx.build(Titled { user_id, title: Some("Hello".into()), body: body.map(Into::into), ..Titled::new_record() })
}

fn errors_on(ctx: &mut Ctx, record: Handle<Titled>, attribute: &str) -> Vec<String> {
    ctx.is_valid(record).unwrap();
    ctx.errors(record).on(attribute).into_iter().map(String::from).collect()
}

#[test]
fn test_uniqueness_within_a_scope() {
    let mut ctx = support::ctx();
    let alice = insert_user(&mut ctx, "alice@example.com");
    let bob = insert_user(&mut ctx, "bob@example.com");
    let sql = "INSERT INTO posts (user_id, title, created_at, updated_at) VALUES ($1, 'Hello', now(), now()) RETURNING id";
    let id: i64 = ctx.query(sql, &[Value::Int(alice)]).unwrap()[0].get(0);
    let same = titled(&mut ctx, Some(alice), Some("ok"));
    assert_eq!(vec!["has already been taken"], errors_on(&mut ctx, same, "title"));
    let other = titled(&mut ctx, Some(bob), Some("ok"));
    assert!(errors_on(&mut ctx, other, "title").is_empty());
    // A nil scope value matches NULL, as `where(user_id: nil)` does; no row has one.
    let orphan = titled(&mut ctx, None, Some("ok"));
    assert!(errors_on(&mut ctx, orphan, "title").is_empty());
    // The record itself doesn't count.
    let saved = Titled::find(&mut ctx, id).unwrap();
    assert!(errors_on(&mut ctx, saved, "title").is_empty());
}

#[test]
fn test_allow_nil_and_allow_blank_skip_a_check() {
    let mut ctx = support::ctx();
    let cases: [(Option<&str>, &[&str]); 4] = [
        (None, &[]),
        (Some(""), &["can't be blank"]),
        (Some("  "), &["can't be blank"]),
        (Some("a"), &["is too short (minimum is 2 characters)"]),
    ];
    for (body, expected) in cases {
        let record = titled(&mut ctx, None, body);
        assert_eq!(expected, errors_on(&mut ctx, record, "body"), "{body:?}");
    }
}
