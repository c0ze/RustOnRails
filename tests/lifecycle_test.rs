mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Ctx, Error, Handle, Model, Record, Time, Value, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::<Person>::new()
                .before_destroy(|ctx, p| if ctx[p].name.as_deref() == Some("keep") { Err(Error::Abort) } else { Ok(()) })
        });
        &BEHAVIOR
    }
}

model! {
    pub struct Counted in "posts" {
        id: i64, user_id: i64, title: String, body: String, status: i64 = 0, published_at: Time,
        comments_count: i64 = 0, created_at: Time, updated_at: Time,
    }
}

impl Model for Counted {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Counted>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

fn saved(ctx: &mut Ctx, name: &str) -> Handle<Person> {
    Person::create_bang(ctx, Person { name: Some(name.into()), email: Some(format!("{name}@example.com")), ..Person::new_record() }).unwrap()
}

#[test]
fn test_destroy_deletes_the_row() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "ann");
    assert!(ctx.destroy(person).unwrap());
    assert!(ctx.is_destroyed(person));
    assert!(!ctx.is_persisted(person));
    assert_eq!(0, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_before_destroy_abort_keeps_the_row() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "keep");
    assert!(!ctx.destroy(person).unwrap());
    assert!(!ctx.is_destroyed(person));
    assert!(matches!(ctx.destroy_bang(person), Err(Error::RecordNotDestroyed { .. })));
    assert_eq!(1, Person::all().count(&mut ctx).unwrap());
}

#[test]
fn test_reload_discards_unsaved_changes() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "ann");
    ctx[person].name = Some("changed".into());
    ctx.reload(person).unwrap();
    assert_eq!(Some("ann"), ctx[person].name.as_deref());
    assert!(ctx.changed(person).is_empty());
}

#[test]
fn test_reload_of_a_deleted_row_raises() {
    let mut ctx = support::ctx();
    let person = saved(&mut ctx, "ann");
    ctx.execute("DELETE FROM users", &[]).unwrap();
    assert!(matches!(ctx.reload(person), Err(Error::RecordNotFound { .. })));
}

#[test]
fn test_increment_bang_updates_memory_and_database() {
    let mut ctx = support::ctx();
    let owner = saved(&mut ctx, "ann");
    let user_id = ctx[owner].id;
    let post = Counted::create_bang(&mut ctx, Counted { user_id, title: Some("t".into()), ..Counted::new_record() }).unwrap();
    let stamp = ctx[post].updated_at;
    ctx.increment_bang(post, "comments_count", 1).unwrap();
    assert_eq!(Some(1), ctx[post].comments_count);
    assert!(ctx.changed(post).is_empty());
    ctx.reload(post).unwrap();
    assert_eq!(Some(1), ctx[post].comments_count);
    assert_eq!(stamp, ctx[post].updated_at);
}

#[test]
fn test_increment_bang_on_a_new_record_raises() {
    let mut ctx = support::ctx();
    let post = ctx.build(Counted::new_record());
    assert!(matches!(ctx.increment_bang(post, "comments_count", 1), Err(Error::NotPersisted { .. })));
}

#[test]
fn test_insert_skips_validations_and_callbacks_but_fills_timestamps() {
    let mut ctx = support::ctx();
    let id = Person::insert(&mut ctx, Person { name: Some("keep".into()), email: Some("k@example.com".into()), ..Person::new_record() }).unwrap();
    let rows = ctx.query("SELECT created_at IS NOT NULL FROM users WHERE id = $1", &[Value::Int(id)]).unwrap();
    assert!(rows[0].get::<_, bool>(0));
}

/// `==` on records is Active Record's: the same id, whichever load it came
/// from; two new records are equal only to themselves.
#[test]
fn test_same_record_compares_ids_like_active_record() {
    let mut ctx = support::ctx();
    let ann = ctx.build(Person { name: Some("Ann".into()), email: Some("ann@example.com".into()), ..Person::new_record() });
    let bob = ctx.build(Person { name: Some("Bob".into()), email: Some("bob@example.com".into()), ..Person::new_record() });
    assert!(ctx.same_record(ann, ann));
    assert!(!ctx.same_record(ann, bob));
    ctx.save_bang(ann).unwrap();
    let id = ctx[ann].id.unwrap();
    let again = Person::find(&mut ctx, id).unwrap();
    assert_ne!(ann, again);
    assert!(ctx.same_record(ann, again));
    assert!(ctx.same_record(Some(ann), Some(again)));
    assert!(!ctx.same_record(Some(ann), None));
    assert!(ctx.same_record(None::<Handle<Person>>, None));
}
