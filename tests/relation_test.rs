mod blog;
mod support;

use blog::{ApplicationRecordScopes, Post, PostScopes, User};
use rustonrails::{Ctx, Error, Model, Value, now};

fn user(ctx: &mut Ctx, email: &str) -> i64 {
    let rows = ctx
        .query(
            "INSERT INTO users (name, email, created_at, updated_at) VALUES ('U', $1, now(), now()) RETURNING id",
            &[Value::from(email)],
        )
        .unwrap();
    rows[0].get(0)
}

fn post(ctx: &mut Ctx, user_id: i64, title: &str, status: i64, days_ago: i64) -> i64 {
    let created = now() - chrono::TimeDelta::days(days_ago);
    let rows = ctx
        .query(
            "INSERT INTO posts (user_id, title, status, created_at, updated_at) VALUES ($1, $2, $3, $4, $4) RETURNING id",
            &[Value::Int(user_id), Value::from(title), Value::Int(status), Value::Time(created)],
        )
        .unwrap();
    rows[0].get(0)
}

#[test]
fn test_find_loads_a_record_with_enum_labels() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    let id = post(&mut ctx, alice, "Hello", 1, 0);
    let found = Post::find(&mut ctx, id).unwrap();
    assert_eq!(Some("Hello"), ctx[found].title.as_deref());
    assert_eq!(Some("published"), ctx[found].status.as_deref());
    assert!(ctx.is_persisted(found));
    assert!(ctx.changed(found).is_empty());
}

#[test]
fn test_find_missing_raises_record_not_found() {
    let mut ctx = support::ctx();
    let error = Post::find(&mut ctx, 0).unwrap_err();
    assert!(matches!(error, Error::RecordNotFound { .. }));
    assert_eq!("Couldn't find Post with 'id'=0", error.to_string());
}

#[test]
fn test_find_by_and_bang() {
    let mut ctx = support::ctx();
    user(&mut ctx, "bob@example.com");
    let bob = User::find_by(&mut ctx, "email", "bob@example.com").unwrap().unwrap();
    assert_eq!(Some("bob@example.com"), ctx[bob].email.as_deref());
    assert!(User::find_by(&mut ctx, "email", "nobody@example.com").unwrap().is_none());
    let error = User::find_by_bang(&mut ctx, "email", "nobody@example.com").unwrap_err();
    assert_eq!("Couldn't find User", error.to_string());
}

#[test]
fn test_scopes_chain_like_ruby() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    let old = post(&mut ctx, alice, "Old", 1, 3);
    let new = post(&mut ctx, alice, "New", 1, 1);
    post(&mut ctx, alice, "Draft", 0, 0);
    let visible = Post::all().visible().recent().load(&mut ctx).unwrap();
    let ids: Vec<Option<i64>> = visible.iter().map(|p| ctx[*p].id).collect();
    assert_eq!(vec![Some(new), Some(old)], ids);
    assert_eq!(1, Post::all().visible().recent().limit(1).load(&mut ctx).unwrap().len());
}

#[test]
fn test_inherited_scope_filters_by_time() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "Old", 1, 3);
    let new = post(&mut ctx, alice, "New", 1, 1);
    let since = now() - chrono::TimeDelta::days(2);
    let recent = Post::all().created_since(since).load(&mut ctx).unwrap();
    assert_eq!(vec![Some(new)], recent.iter().map(|p| ctx[*p].id).collect::<Vec<_>>());
}

#[test]
fn test_count_exists_first() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    let first = post(&mut ctx, alice, "A", 0, 0);
    post(&mut ctx, alice, "B", 0, 0);
    assert_eq!(2, Post::all().count(&mut ctx).unwrap());
    assert_eq!(1, Post::all().limit(1).count(&mut ctx).unwrap());
    assert!(Post::all().where_eq("title", "B").exists(&mut ctx).unwrap());
    assert!(!Post::all().where_eq("title", "C").exists(&mut ctx).unwrap());
    let loaded = Post::all().first(&mut ctx).unwrap().unwrap();
    assert_eq!(Some(first), ctx[loaded].id);
}

#[test]
fn test_unknown_enum_label_matches_nothing() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "A", 0, 0);
    assert_eq!(0, Post::all().where_eq("status", "archived").count(&mut ctx).unwrap());
    let (sql, params) = Post::all().where_eq("status", "published").to_sql();
    assert!(sql.contains("\"posts\".\"status\" = $1"), "{sql}");
    assert_eq!(vec![Value::Int(1)], params);
}

#[test]
fn test_values_are_parameters_not_sql() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "It's \"quoted\"; DROP TABLE posts", 0, 0);
    let found = Post::find_by(&mut ctx, "title", "It's \"quoted\"; DROP TABLE posts").unwrap();
    assert!(found.is_some());
    assert_eq!(1, Post::all().count(&mut ctx).unwrap());
}

#[test]
fn test_where_not_and_nil() {
    let mut ctx = support::ctx();
    let alice = user(&mut ctx, "alice@example.com");
    post(&mut ctx, alice, "A", 0, 0);
    post(&mut ctx, alice, "B", 1, 0);
    assert_eq!(1, Post::all().where_not("status", "draft").count(&mut ctx).unwrap());
    assert_eq!(2, Post::all().where_eq("published_at", None::<rustonrails::Time>).count(&mut ctx).unwrap());
}
