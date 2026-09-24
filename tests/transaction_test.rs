mod support;

use rustonrails::{Error, Value};

fn count(ctx: &mut rustonrails::Ctx) -> i64 {
    ctx.query("SELECT COUNT(*) FROM users", &[]).unwrap()[0].get(0)
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

#[test]
fn test_nested_failure_keeps_outer_work() {
    let mut ctx = support::ctx();
    let outer = ctx.transaction(|ctx| {
        insert_user(ctx, "kept@example.com");
        let inner = ctx.transaction(|ctx| { insert_user(ctx, "dropped@example.com"); Err(Error::Abort) });
        assert!(matches!(inner, Err(Error::Abort)));
        Ok(true)
    });
    assert!(outer.unwrap());
    assert_eq!(1, count(&mut ctx));
}

#[test]
fn test_database_errors_surface() {
    let mut ctx = support::ctx();
    let result = ctx.execute("INSERT INTO users (name) VALUES ($1)", &[Value::from("no email")]);
    assert!(matches!(result, Err(Error::Db(_))));
}
