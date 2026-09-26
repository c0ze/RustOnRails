mod support;

use rustonrails::{Error, Value};

/// The users this test's `Ctx` inserted: the one test that commits uses
/// emails starting with "commit-", which the others must not count.
fn count(ctx: &mut rustonrails::Ctx) -> i64 {
    ctx.query("SELECT COUNT(*) FROM users WHERE email NOT LIKE 'commit-%'", &[]).unwrap()[0].get(0)
}

fn insert_user(ctx: &mut rustonrails::Ctx, email: &str) {
    ctx.execute(
        "INSERT INTO users (name, email, created_at, updated_at) VALUES ($1, $2, now(), now())",
        &[Value::from("Test"), Value::from(email)],
    )
    .unwrap();
}

#[test]
fn test_each_ctx_sees_only_its_own_rows() {
    let mut first = support::ctx();
    let mut second = support::ctx();
    insert_user(&mut first, "first@example.com");
    assert_eq!(1, count(&mut first));
    assert_eq!(0, count(&mut second));
}

#[test]
fn test_transaction_returning_false_rolls_back() {
    let mut ctx = support::ctx();
    let kept = ctx.transaction(|ctx| { insert_user(ctx, "gone@example.com"); Ok(false) }).unwrap();
    assert!(!kept);
    assert_eq!(0, count(&mut ctx));
}

/// The test's transaction can't be joined, like Rails' fixture
/// transaction, so the first transaction inside it is a savepoint of its
/// own; one inside that joins it, as in Rails, and a failure there rolls
/// nothing back until the outer one ends.
#[test]
fn test_a_nested_transaction_joins_the_outer_one() {
    let mut ctx = support::ctx();
    let outer = ctx.transaction(|ctx| {
        insert_user(ctx, "kept@example.com");
        let inner = ctx.transaction(|ctx| { insert_user(ctx, "joined@example.com"); Ok(false) });
        assert!(!inner.unwrap());
        Ok(true)
    });
    assert!(outer.unwrap());
    assert_eq!(2, count(&mut ctx));

    let failed = ctx.transaction(|ctx| {
        insert_user(ctx, "gone@example.com");
        ctx.transaction(|ctx| { insert_user(ctx, "gone too@example.com"); Err(Error::Abort) })
    });
    assert!(matches!(failed, Err(Error::Abort)));
    assert_eq!(2, count(&mut ctx));
}

#[test]
fn test_a_transaction_block_gives_its_value() {
    let mut ctx = support::ctx();
    let value = ctx.transaction_block(|ctx| { insert_user(ctx, "a@example.com"); Ok(42) }).unwrap();
    assert_eq!(Some(42), value);
    assert_eq!(1, count(&mut ctx));
}

/// `raise ActiveRecord::Rollback` rolls the block back and gives nil;
/// the code around it goes on.
#[test]
fn test_rollback_gives_nil_and_rolls_back() {
    let mut ctx = support::ctx();
    let value: Option<i64> = ctx.transaction_block(|ctx| { insert_user(ctx, "a@example.com"); Err(Error::Rollback) }).unwrap();
    assert_eq!(None, value);
    assert_eq!(0, count(&mut ctx));
    insert_user(&mut ctx, "b@example.com");
    assert_eq!(1, count(&mut ctx));
}

/// Any other error rolls back and goes on up.
#[test]
fn test_an_error_rolls_back_and_propagates() {
    let mut ctx = support::ctx();
    let outcome: Result<Option<()>, Error> =
        ctx.transaction_block(|ctx| { insert_user(ctx, "a@example.com"); Err(Error::RecordNotSaved { model: "User" }) });
    assert!(matches!(outcome, Err(Error::RecordNotSaved { .. })));
    assert_eq!(0, count(&mut ctx));
}

/// Inside another transaction block, Rollback is swallowed where it's
/// raised and rolls nothing back: the outer block commits it all.
#[test]
fn test_rollback_in_a_joined_block_rolls_nothing_back() {
    let mut ctx = support::ctx();
    let outer = ctx.transaction_block(|ctx| {
        insert_user(ctx, "outer@example.com");
        let inner: Option<()> = ctx.transaction_block(|ctx| { insert_user(ctx, "inner@example.com"); Err(Error::Rollback) })?;
        assert_eq!(None, inner);
        Ok("done")
    });
    assert_eq!(Some("done"), outer.unwrap());
    assert_eq!(2, count(&mut ctx));
}

/// A save that fails inside a transaction block joins it: the block's
/// other writes stay, as in Rails.
#[test]
fn test_a_failed_save_in_a_block_keeps_the_block_going() {
    let mut ctx = support::ctx();
    let value = ctx.transaction_block(|ctx| {
        insert_user(ctx, "a@example.com");
        let saved = ctx.transaction(|ctx| { insert_user(ctx, "b@example.com"); Ok(false) })?;
        Ok(saved)
    });
    assert_eq!(Some(false), value.unwrap());
    assert_eq!(2, count(&mut ctx));
}

/// Outside any transaction, a block commits with BEGIN and COMMIT.
#[test]
fn test_a_block_outside_a_transaction_commits() {
    support::prepare();
    let url = support::url();
    let mut ctx = rustonrails::Ctx::connect(&url).unwrap();
    let email = format!("commit-{}@example.com", std::process::id());
    let value = ctx.transaction_block(|ctx| { insert_user(ctx, &email); Ok(1) }).unwrap();
    assert_eq!(Some(1), value);
    let mut other = rustonrails::Ctx::connect(&url).unwrap();
    let found = other.query("SELECT COUNT(*) FROM users WHERE email = $1", &[Value::from(email.as_str())]).unwrap();
    assert_eq!(1i64, found[0].get::<_, i64>(0));
    other.execute("DELETE FROM users WHERE email = $1", &[Value::from(email.as_str())]).unwrap();
    let rolled: Option<()> = ctx.transaction_block(|ctx| { insert_user(ctx, &email); Err(Error::Rollback) }).unwrap();
    assert_eq!(None, rolled);
    let found = other.query("SELECT COUNT(*) FROM users WHERE email = $1", &[Value::from(email.as_str())]).unwrap();
    assert_eq!(0i64, found[0].get::<_, i64>(0));
}

#[test]
fn test_database_errors_surface() {
    let mut ctx = support::ctx();
    let result = ctx.execute("INSERT INTO users (name) VALUES ($1)", &[Value::from("no email")]);
    assert!(matches!(result, Err(Error::Db(_))));
}

#[test]
fn test_database_errors_keep_the_postgres_message() {
    let mut ctx = support::ctx();
    let error = ctx.execute("INSERT INTO users (name) VALUES ($1)", &[Value::from("no email")]).unwrap_err();
    assert!(error.to_string().contains("null value in column \"email\""), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}
