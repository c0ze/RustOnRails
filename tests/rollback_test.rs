//! What a rollback does to the records it touched, and a Rollback a
//! callback raises, as Active Record 8.1 does both.

mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Ctx, Error, Handle, Model, Record, Time, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(|| {
            Behavior::<Person>::new()
                .before_save(|ctx, p| if ctx[p].name.as_deref() == Some("rollback") { Err(Error::Rollback) } else { Ok(()) })
                .before_destroy(|ctx, p| if ctx[p].name.as_deref() == Some("keep") { Err(Error::Rollback) } else { Ok(()) })
        });
        &BEHAVIOR
    }
}

fn person(ctx: &mut Ctx, name: &str) -> Handle<Person> {
    let record = Person { name: Some(name.into()), email: Some(format!("{name}@example.com")), ..Person::new_record() };
    ctx.build(record)
}

fn count(ctx: &mut Ctx, name: &str) -> i64 {
    Person::all().where_eq("name", name).count(ctx).unwrap()
}

/// A record created in a rolled-back block is new again, so saving it
/// later inserts it.
#[test]
fn test_a_created_record_is_new_again_after_a_rollback() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "ann");
    let rolled: Option<()> = ctx.transaction_block(|ctx| {
        ctx.save_bang(p)?;
        Err(Error::Rollback)
    }).unwrap();
    assert_eq!(None, rolled);
    assert!(ctx.is_new_record(p));
    assert_eq!(None, ctx[p].id);
    ctx.save_bang(p).unwrap();
    assert_eq!(1, count(&mut ctx, "ann"));
}

/// An updated record keeps its new values, which count as changes again.
#[test]
fn test_an_updated_record_is_changed_again_after_a_rollback() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "bob");
    ctx.save_bang(p).unwrap();
    let _: Option<()> = ctx.transaction_block(|ctx| {
        ctx[p].name = Some("robert".into());
        ctx.save_bang(p)?;
        Err(Error::Rollback)
    }).unwrap();
    assert_eq!(Some("robert".to_string()), ctx[p].name);
    assert_eq!(vec!["name"], ctx.changed(p).into_iter().filter(|c| *c == "name").collect::<Vec<_>>());
    ctx.save_bang(p).unwrap();
    assert_eq!(1, count(&mut ctx, "robert"));
}

/// A destroyed record isn't, after a rollback.
#[test]
fn test_a_destroyed_record_comes_back_after_a_rollback() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "cy");
    ctx.save_bang(p).unwrap();
    let _: Option<()> = ctx.transaction_block(|ctx| {
        ctx.destroy_bang(p)?;
        Err(Error::Rollback)
    }).unwrap();
    assert!(!ctx.is_destroyed(p));
    assert!(ctx.is_persisted(p));
}

/// A Rollback raised in a callback makes save false (save! returns),
/// and in a joined transaction rolls nothing back.
#[test]
fn test_a_rollback_in_a_callback() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "rollback");
    assert!(!ctx.save(p).unwrap());
    assert!(ctx.save_bang(p).is_ok());
    assert!(ctx.is_new_record(p));

    let kept: Option<bool> = ctx.transaction_block(|ctx| {
        let outer = person(ctx, "outer");
        ctx.save_bang(outer)?;
        let inner = person(ctx, "rollback");
        ctx.save(inner)
    }).unwrap();
    assert_eq!(Some(false), kept);
    assert_eq!(1, count(&mut ctx, "outer"));

    let keep = person(&mut ctx, "keep");
    ctx.save_bang(keep).unwrap();
    assert!(!ctx.destroy(keep).unwrap());
    assert!(matches!(ctx.destroy_bang(keep), Err(Error::RecordNotDestroyed { .. })));
    assert_eq!(1, count(&mut ctx, "keep"));
}

/// A relation that loaded answers `size`, `any?`, `first` and `pluck`
/// from its records; `count` still asks the database.
#[test]
fn test_a_loaded_relation_uses_its_records() {
    let mut ctx = support::ctx();
    for name in ["dee", "eve"] {
        let p = person(&mut ctx, name);
        ctx.save_bang(p).unwrap();
    }
    let named = Person::all().where_in("name", vec!["dee".into(), "eve".into()]).order_asc("name");
    let loaded = named.load(&mut ctx).unwrap();
    ctx.execute("UPDATE users SET name = 'gone' WHERE name IN ('dee', 'eve')", &[]).unwrap();
    assert_eq!(2, named.size(&mut ctx).unwrap());
    assert!(named.is_any(&mut ctx).unwrap());
    assert_eq!(Some(loaded[0]), named.first(&mut ctx).unwrap());
    assert_eq!(vec![Some("dee".to_string()), Some("eve".into())], named.pluck::<String>(&mut ctx, "name").unwrap());
    assert_eq!(0, named.count(&mut ctx).unwrap());
    // Anything built from it is a new query.
    assert_eq!(0, named.clone().limit(5).size(&mut ctx).unwrap());
    assert!(!named.clone().is_any(&mut ctx).unwrap());
}

#[test]
fn test_limit_zero_has_no_first() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "fay");
    ctx.save_bang(p).unwrap();
    assert_eq!(None, Person::all().limit(0).first(&mut ctx).unwrap());
}

/// `none`, what an owner that was never saved has many of.
#[test]
fn test_none_matches_nothing() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "gil");
    ctx.save_bang(p).unwrap();
    assert!(Person::all().none().to_sql().0.ends_with("WHERE (1=0)"));
    assert_eq!(0, Person::all().none().count(&mut ctx).unwrap());
}

/// A record saved before the block isn't the block's to put back.
#[test]
fn test_records_the_block_didnt_touch_are_left_alone() {
    let mut ctx = support::ctx();
    let p = person(&mut ctx, "hal");
    ctx.save_bang(p).unwrap();
    let _: Option<()> = ctx.transaction_block(|_| Err(Error::Rollback)).unwrap();
    assert!(ctx.is_persisted(p));
}
