mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Check, Ctx, Error, Handle, Model, Record, Result, Time, errors_json, model, now};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::<Person>::new()
                .validates("name", Check::Presence)
                .before_save(|ctx, p| log(ctx, p, "before_save"))
                .before_create(|ctx, p| log(ctx, p, "before_create"))
                .after_create(|ctx, p| log(ctx, p, "after_create"))
                .after_save(|ctx, p| log(ctx, p, "after_save"))
                .before_update(|ctx, p| log(ctx, p, "before_update"))
                .after_update(|ctx, p| log(ctx, p, "after_update"))
                .after_create(|ctx, p| if ctx[p].name.as_deref() == Some("explode") { Err(Error::Nil { what: "explode" }) } else { Ok(()) })
                .before_save(|ctx, p| if ctx[p].name.as_deref() == Some("halt") { Err(Error::Abort) } else { Ok(()) })
        });
        &BEHAVIOR
    }
}

model! {
    pub struct Plain in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Plain {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Plain>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

// An enum without `validate: true`: Rails raises on an unknown label.
model! {
    pub struct Loose in "posts" {
        id: i64, user_id: i64, title: String, body: String, status: String = "draft", published_at: Time,
        comments_count: i64 = 0, created_at: Time, updated_at: Time,
    }
}

impl Model for Loose {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Loose>> =
            LazyLock::new(|| Behavior::<Loose>::new().enumeration("status", &[("draft", 0), ("published", 1)], false));
        &BEHAVIOR
    }
}

// `before_save :stamp, if: :will_save_change_to_status?`, as Rutile compiles it.
model! {
    pub struct Stamped in "posts" {
        id: i64, user_id: i64, title: String, status: String = "draft", published_at: Time, created_at: Time, updated_at: Time,
    }
}

impl Model for Stamped {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Stamped>> = LazyLock::new(|| {
            Behavior::<Stamped>::new()
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
                .before_save(|ctx, post| {
                    ctx[post].published_at = ctx[post].is_published().then(now);
                    Ok(())
                })
                .when(|ctx, post| ctx.attribute_changed(post, "status"))
        });
        &BEHAVIOR
    }
}

impl Stamped {
    fn is_published(&self) -> bool {
        self.status.as_deref() == Some("published")
    }
}

/// Callbacks append to the email so tests can read the order they ran in.
fn log(ctx: &mut Ctx, person: Handle<Person>, step: &str) -> Result<()> {
    let email = ctx[person].email.get_or_insert_with(String::new);
    email.push_str(step);
    email.push(' ');
    Ok(())
}

fn build(ctx: &mut Ctx, name: &str) -> Handle<Person> {
    ctx.build(Person { name: Some(name.into()), ..Person::new_record() })
}

#[test]
fn test_create_runs_callbacks_in_order_and_sets_timestamps() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "Ann");
    assert!(ctx.save(person).unwrap());
    assert!(ctx.is_persisted(person));
    assert!(ctx[person].id.is_some());
    assert_eq!(Some("before_save before_create after_create after_save "), ctx[person].email.as_deref());
    assert_eq!(ctx[person].created_at, ctx[person].updated_at);
    assert!(ctx[person].created_at.is_some());
}

#[test]
fn test_update_writes_only_changes_and_bumps_updated_at() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "Ann");
    ctx.save_bang(person).unwrap();
    let created = ctx[person].updated_at;
    std::thread::sleep(std::time::Duration::from_millis(2));
    ctx[person].email = None;
    assert!(ctx.update(person, |p| p.name = Some("Bea".into())).unwrap());
    assert_eq!(Some("before_save before_update after_update after_save "), ctx[person].email.as_deref());
    assert!(ctx[person].updated_at > created);
    // Rails applies changes before the after callbacks, so their edits stay unsaved.
    assert_eq!(vec!["email"], ctx.changed(person));
    let id = ctx[person].id.unwrap();
    let reloaded = Person::find(&mut ctx, id).unwrap();
    assert_eq!(Some("Bea"), ctx[reloaded].name.as_deref());
}

#[test]
fn test_save_without_changes_skips_the_update() {
    let mut ctx = support::ctx();
    let plain = Plain::create_bang(&mut ctx, Plain { name: Some("Ann".into()), email: Some("ann@example.com".into()), ..Plain::new_record() }).unwrap();
    let stamp = ctx[plain].updated_at;
    ctx.execute("UPDATE users SET name = 'from the database'", &[]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert!(ctx.save(plain).unwrap());
    assert_eq!(stamp, ctx[plain].updated_at);
    let rows = ctx.query("SELECT name FROM users", &[]).unwrap();
    assert_eq!("from the database", rows[0].get::<_, String>(0));
}

#[test]
fn test_invalid_save_returns_false_and_bang_raises() {
    let mut ctx = support::ctx();
    let nameless = ctx.build(Person::new_record());
    assert!(!ctx.save(nameless).unwrap());
    assert!(ctx.is_new_record(nameless));
    let error = ctx.save_bang(nameless).unwrap_err();
    assert_eq!("Validation failed: Name can't be blank", error.to_string());
    let created = Person::create(&mut ctx, Person::new_record()).unwrap();
    assert!(ctx.is_new_record(created));
    assert!(matches!(Person::create_bang(&mut ctx, Person::new_record()), Err(Error::RecordInvalid { .. })));
}

/// `error.record.errors` in a rescue handler: RecordInvalid carries them.
#[test]
fn test_record_invalid_carries_the_errors() {
    let mut ctx = support::ctx();
    let nameless = ctx.build(Person::new_record());
    let Err(Error::RecordInvalid(invalid)) = ctx.save_bang(nameless) else { panic!("expected RecordInvalid") };
    assert_eq!("Person", invalid.model);
    assert_eq!(ctx.errors(nameless), &invalid.errors);
    assert_eq!(r#"{"name":["can't be blank"]}"#, errors_json(&invalid.errors).to_string());
}

#[test]
fn test_before_callback_abort_halts_the_save() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "halt");
    assert!(!ctx.save(person).unwrap());
    assert!(ctx.is_new_record(person));
    assert!(matches!(ctx.save_bang(person), Err(Error::RecordNotSaved { .. })));
    assert_eq!(0, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_failed_after_create_rolls_back_and_leaves_record_new() {
    let mut ctx = support::ctx();
    // Plain has no logging callbacks, so its email can't collide with the next insert's.
    Plain::create_bang(&mut ctx, Plain { name: Some("Kept".into()), email: Some("kept@example.com".into()), ..Plain::new_record() }).unwrap();
    let person = build(&mut ctx, "explode");
    assert!(matches!(ctx.save(person), Err(Error::Nil { .. })));
    assert!(ctx.is_new_record(person));
    assert_eq!(None, ctx[person].id);
    assert_eq!(1, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_timestamps_survive_reload() {
    let mut ctx = support::ctx();
    let person = build(&mut ctx, "Ann");
    ctx.save_bang(person).unwrap();
    let id = ctx[person].id.unwrap();
    let found = Person::find(&mut ctx, id).unwrap();
    assert_eq!(ctx[person].created_at, ctx[found].created_at);
}

/// `will_save_change_to_status?`: a new record's default isn't a change,
/// an assigned value is until it's saved.
#[test]
fn test_a_callback_on_will_save_change_to() {
    let mut ctx = support::ctx();
    let owner = Plain::create_bang(&mut ctx, Plain { name: Some("Ann".into()), email: Some("ann@example.com".into()), ..Plain::new_record() }).unwrap();
    let long_ago = chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap().and_hms_opt(0, 0, 0);
    let post = Stamped { user_id: ctx[owner].id, title: Some("t".into()), published_at: long_ago, ..Stamped::new_record() };
    let post = Stamped::create_bang(&mut ctx, post).unwrap();
    assert_eq!(long_ago, ctx[post].published_at);
    ctx.update_bang(post, |p| p.status = Some("published".into())).unwrap();
    let stamped = ctx[post].published_at;
    assert!(stamped > long_ago);
    assert!(!ctx.attribute_changed(post, "status"));
    ctx.update_bang(post, |p| p.published_at = long_ago).unwrap();
    assert_eq!(long_ago, ctx[post].published_at);
}

#[test]
fn test_unknown_enum_label_raises_on_write() {
    let mut ctx = support::ctx();
    let owner = Plain::create_bang(&mut ctx, Plain { name: Some("Ann".into()), email: Some("ann@example.com".into()), ..Plain::new_record() }).unwrap();
    let user_id = ctx[owner].id;
    let error = Loose::create(&mut ctx, Loose { user_id, title: Some("t".into()), status: Some("archived".into()), ..Loose::new_record() }).unwrap_err();
    assert!(matches!(error, Error::InvalidEnum { .. }), "{error:?}");
    assert_eq!("'archived' is not a valid status", error.to_string());
}

/// A count too big for a bigint is validated as given, then refused at the
/// write, as Rails raises ActiveModel::RangeError, instead of saving 0.
#[test]
fn test_an_integer_ruby_would_make_a_bignum_is_not_written() {
    let mut ctx = support::ctx();
    let owner = Plain::create_bang(&mut ctx, Plain { name: Some("Ann".into()), email: Some("ann@example.com".into()), ..Plain::new_record() }).unwrap();
    let user_id = ctx[owner].id;
    let post = Loose::create_bang(&mut ctx, Loose { user_id, title: Some("t".into()), ..Loose::new_record() }).unwrap();
    ctx.assign(post, &[("comments_count".into(), "99999999999999999999".into())]).unwrap();
    let error = ctx.save(post).unwrap_err();
    assert!(matches!(error, Error::Overflow { .. }), "{error:?}");
    let id = ctx[post].id.unwrap();
    let stored = ctx.query("SELECT comments_count FROM posts WHERE id = $1", &[id.into()]).unwrap();
    assert_eq!(0, stored[0].get::<_, i32>(0));
    let fresh = Loose::create(&mut ctx, Loose { user_id, title: Some("u".into()), ..Loose::new_record() }).unwrap();
    ctx.assign(fresh, &[("comments_count".into(), 1e20.into())]).unwrap();
    assert!(matches!(ctx.save(fresh), Err(Error::Overflow { .. })));
}
