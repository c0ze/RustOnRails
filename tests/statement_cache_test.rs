mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Ctx, Model, Time, Value, model};

model! {
    pub struct Person in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

impl Model for Person {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Person>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

/// The statements Postgres holds prepared for this session. The count
/// query is one of them, prepared the first time it runs.
fn prepared(ctx: &mut Ctx) -> i64 {
    ctx.query("SELECT count(*)::bigint FROM pg_prepared_statements", &[]).unwrap()[0].get(0)
}

/// A query runs as a statement prepared once per connection, as Rails'
/// statement cache does; the statements outlive the Ctx that made them.
#[test]
fn test_a_query_is_prepared_once_per_connection() {
    let mut ctx = support::ctx();
    let before = prepared(&mut ctx);
    for name in ["ann", "bob", "cy"] {
        Person::all().where_eq("name", name).load(&mut ctx).unwrap();
    }
    assert_eq!(before + 1, prepared(&mut ctx));

    let mut again = Ctx::resume(ctx.into_connection());
    Person::all().where_eq("name", "dee").load(&mut again).unwrap();
    assert_eq!(before + 1, prepared(&mut again));
}

/// A fragment's binds are written into its SQL, so each value would be a
/// statement of its own: those run unprepared, as in Rails.
#[test]
fn test_a_sql_fragment_isnt_kept() {
    let mut ctx = support::ctx();
    let before = prepared(&mut ctx);
    for pattern in ["%a%", "%b%", "%c%"] {
        Person::all().where_sql("name ILIKE ?", vec![Value::from(pattern)]).load(&mut ctx).unwrap();
        Person::all().where_sql("name ILIKE ?", vec![Value::from(pattern)]).count(&mut ctx).unwrap();
    }
    assert_eq!(before, prepared(&mut ctx));
}

/// Past Rails' limit of 1000 the oldest statement goes.
#[test]
fn test_the_cache_keeps_a_thousand_statements() {
    let mut ctx = support::ctx();
    for n in 0..1005 {
        ctx.query(&format!("SELECT {n}::bigint"), &[]).unwrap();
    }
    // The count query is one more statement, which pushes out one more;
    // what's pushed out is closed on the server too.
    assert_eq!(1000, prepared(&mut ctx));
}
