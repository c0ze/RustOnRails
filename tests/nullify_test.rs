mod support;

use std::sync::LazyLock;

use rustonrails::{Behavior, Ctx, Error, HasMany, Model, Record, Time, Value, model};

model! {
    pub struct Owner in "users" { id: i64, name: String, email: String, created_at: Time, updated_at: Time }
}

model! {
    pub struct Note in "notes" { id: i64, user_id: i64, body: String, updated_at: Time }
}

impl Owner {
    const NOTES: HasMany<Owner, Note> = HasMany::new("notes", "user_id", None);
}

// `has_many :notes, dependent: :nullify`, then a before_destroy that can
// still stop the destroy.
impl Model for Owner {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Owner>> = LazyLock::new(|| {
            Behavior::<Owner>::new()
                .before_destroy(|ctx, owner| Owner::NOTES.nullify_all(ctx, owner))
                .before_destroy(|ctx, owner| if ctx[owner].name.as_deref() == Some("keep") { Err(Error::Abort) } else { Ok(()) })
        });
        &BEHAVIOR
    }
}

impl Model for Note {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Note>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

/// The blog has no nullable foreign key; this table lives for one test.
fn ctx() -> Ctx {
    let mut ctx = support::ctx();
    let sql = "CREATE TEMP TABLE notes (id bigserial PRIMARY KEY, user_id bigint, body text, updated_at timestamp(6))";
    ctx.execute(sql, &[]).unwrap();
    ctx
}

fn owner(ctx: &mut Ctx, name: &str) -> i64 {
    let sql = "INSERT INTO users (name, email, created_at, updated_at) VALUES ($1, $2, now(), now()) RETURNING id";
    let id = ctx.query(sql, &[Value::from(name), Value::from(format!("{name}@example.com"))]).unwrap()[0].get(0);
    for body in ["a", "b"] {
        let sql = "INSERT INTO notes (user_id, body, updated_at) VALUES ($1, $2, '2020-01-01')";
        ctx.execute(sql, &[Value::Int(id), Value::from(format!("{name} {body}"))]).unwrap();
    }
    id
}

fn owned(ctx: &mut Ctx, id: i64) -> i64 {
    Note::all().where_eq("user_id", id).count(ctx).unwrap()
}

/// One UPDATE: the rows stay, their callbacks don't run, updated_at stays.
#[test]
fn test_destroy_nullifies_the_foreign_key() {
    let mut ctx = ctx();
    let ann = owner(&mut ctx, "ann");
    let bob = owner(&mut ctx, "bob");
    let record = Owner::find(&mut ctx, ann).unwrap();
    ctx.destroy_bang(record).unwrap();
    assert_eq!(0, owned(&mut ctx, ann));
    assert_eq!(2, owned(&mut ctx, bob));
    assert_eq!(4, Note::all().count(&mut ctx).unwrap());
    let stamps = ctx.query("SELECT count(*) FROM notes WHERE updated_at = '2020-01-01'", &[]).unwrap();
    assert_eq!(4, stamps[0].get::<_, i64>(0));
}

#[test]
fn test_an_aborted_destroy_keeps_the_keys() {
    let mut ctx = ctx();
    let keep = owner(&mut ctx, "keep");
    let record = Owner::find(&mut ctx, keep).unwrap();
    assert!(!ctx.destroy(record).unwrap());
    assert_eq!(2, owned(&mut ctx, keep));
}

/// Rails scopes a new record's association to nothing, whatever its id.
#[test]
fn test_a_new_owner_nullifies_nothing() {
    let mut ctx = ctx();
    let ann = owner(&mut ctx, "ann");
    let unsaved = ctx.build(Owner { id: Some(ann), ..Owner::new_record() });
    ctx.destroy(unsaved).unwrap();
    assert_eq!(2, owned(&mut ctx, ann));
}
