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

model! {
    pub struct Code in "rustonrails_deferred_codes" { id: i64, code: String, created_at: Time, updated_at: Time }
}

impl Model for Code {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Code>> = LazyLock::new(Behavior::<Code>::new);
        &BEHAVIOR
    }
}

/// A COMMIT can fail where no statement before it did: a deferred
/// constraint is checked there. The database rolls back, so the records
/// the block saved are new again rather than holding ids no row has.
#[test]
fn test_a_failed_commit_puts_the_records_back() {
    support::prepare();
    let mut ctx = Ctx::connect(&support::url()).unwrap();
    let first = ctx.build(Code { code: Some("same".into()), ..Code::new_record() });
    let second = ctx.build(Code { code: Some("same".into()), ..Code::new_record() });
    let outcome: Result<Option<()>, Error> = ctx.transaction_block(|ctx| {
        // A table of this connection's own, gone with the rollback.
        ctx.execute(
            "CREATE TEMP TABLE rustonrails_deferred_codes (id bigserial PRIMARY KEY, \
             code text UNIQUE DEFERRABLE INITIALLY DEFERRED, created_at timestamp NOT NULL, updated_at timestamp NOT NULL)",
            &[],
        )?;
        ctx.save_bang(first)?;
        ctx.save_bang(second)?;
        Ok(())
    });
    assert!(outcome.is_err(), "the COMMIT fails the deferred unique constraint");
    for record in [first, second] {
        assert!(ctx.is_new_record(record));
        assert!(ctx[record].id.is_none());
    }
}

/// Handles are slots in one `Ctx`: a relation loaded in one and asked in
/// another queries again rather than reading that one's slots.
#[test]
fn test_a_loaded_relation_asked_in_another_ctx_queries_again() {
    let mut a = support::ctx();
    let alice = person(&mut a, "alice-elsewhere");
    a.save_bang(alice).unwrap();
    let relation = Person::all().where_eq("name", "alice-elsewhere");
    assert_eq!(1, relation.load(&mut a).unwrap().len());
    // Another connection, whose transaction doesn't see Alice.
    let mut b = support::ctx();
    let bob = person(&mut b, "bob-elsewhere");
    b.save_bang(bob).unwrap();
    assert_eq!(None, relation.first(&mut b).unwrap());
    assert!(relation.pluck::<String>(&mut b, "name").unwrap().is_empty());
    assert_eq!(0, relation.size(&mut b).unwrap());
    let mut batches = relation.batches(10);
    assert_eq!(None, batches.next(&mut b).unwrap());
}

/// Batches begun from a loaded relation's records and continued in
/// another Ctx go on after the last record given, not from the start.
#[test]
fn test_batches_continue_after_what_they_gave() {
    support::prepare();
    let mut a = Ctx::connect(&support::url()).unwrap();
    let names = ["batch-zero", "batch-one", "batch-two"];
    let saved: Vec<Handle<Person>> = names.iter().map(|name| person(&mut a, name)).collect();
    for p in &saved {
        a.save_bang(*p).unwrap();
    }
    // Newest first, skipping the newest: the window is zero and one,
    // which batches give by id.
    let relation = Person::all()
        .where_in("name", names.iter().map(|n| rustonrails::Value::from(n.to_string())).collect())
        .order_desc("id")
        .offset(1);
    relation.load(&mut a).unwrap();
    let mut batches = relation.batches(1);
    let first = batches.next(&mut a).unwrap().unwrap();
    assert_eq!(Some("batch-zero"), a[first[0]].name.as_deref());
    let mut b = Ctx::connect(&support::url()).unwrap();
    let second = batches.next(&mut b).unwrap().unwrap();
    assert_eq!(Some("batch-one"), b[second[0]].name.as_deref());
    assert_eq!(None, batches.next(&mut b).unwrap());
    for p in saved {
        a.destroy_bang(p).unwrap();
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
