mod fixtures;
mod support;

use fixtures::Fixtures;
use blog::models::{Comment, Post, PostScopes, User};
use rustonrails::{Ctx, Model, Record, Value};

fn setup() -> (Ctx, Fixtures) {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    (ctx, fx)
}

fn rename_users(ctx: &mut Ctx, name: &str) {
    ctx.execute("UPDATE users SET name = $1", &[Value::from(name)]).unwrap();
}

#[test]
fn test_belongs_to_loads_then_caches() {
    let (mut ctx, _) = setup();
    let post = Post::all().visible().recent().first(&mut ctx).unwrap().unwrap();
    let user = Post::USER.get(&mut ctx, post).unwrap().unwrap();
    assert_eq!(Some("Bob"), ctx[user].name.as_deref());
    rename_users(&mut ctx, "Renamed");
    let again = Post::USER.get(&mut ctx, post).unwrap().unwrap();
    assert_eq!(user, again);
    assert_eq!(Some("Bob"), ctx[again].name.as_deref());
}

#[test]
fn test_changing_the_foreign_key_reloads_the_target() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_new).unwrap();
    let bob = Post::USER.get(&mut ctx, post).unwrap().unwrap();
    assert_eq!(Some("Bob"), ctx[bob].name.as_deref());
    ctx[post].user_id = Some(fx.alice);
    let alice = Post::USER.get(&mut ctx, post).unwrap().unwrap();
    assert_eq!(Some("Alice"), ctx[alice].name.as_deref());
    ctx[post].user_id = None;
    assert_eq!(None, Post::USER.get(&mut ctx, post).unwrap());
}

#[test]
fn test_set_assigns_the_key_and_the_cache() {
    let (mut ctx, fx) = setup();
    let comment = ctx.build(Comment::new_record());
    let post = Post::find(&mut ctx, fx.published_new).unwrap();
    Comment::POST.set(&mut ctx, comment, post).unwrap();
    assert_eq!(Some(fx.published_new), ctx[comment].post_id);
    assert_eq!(Some(post), Comment::POST.get(&mut ctx, comment).unwrap());
}

#[test]
fn test_includes_preloads_with_one_record_per_target() {
    let (mut ctx, fx) = setup();
    let posts = Post::all().where_eq("user_id", fx.alice).includes(&Post::USER).load(&mut ctx).unwrap();
    assert_eq!(2, posts.len());
    rename_users(&mut ctx, "Renamed");
    let first = Post::USER.get(&mut ctx, posts[0]).unwrap().unwrap();
    let second = Post::USER.get(&mut ctx, posts[1]).unwrap().unwrap();
    assert_eq!(first, second);
    assert_eq!(Some("Alice"), ctx[first].name.as_deref());
}

#[test]
fn test_where_in() {
    let (mut ctx, fx) = setup();
    assert_eq!(2, Post::all().where_in("id", vec![Value::Int(fx.draft), Value::Int(fx.published_new)]).count(&mut ctx).unwrap());
    assert_eq!(0, Post::all().where_in("id", vec![]).count(&mut ctx).unwrap());
}

#[test]
fn test_reload_clears_the_association_cache() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_new).unwrap();
    Post::USER.get(&mut ctx, post).unwrap();
    rename_users(&mut ctx, "Renamed");
    ctx.reload(post).unwrap();
    let user = Post::USER.get(&mut ctx, post).unwrap().unwrap();
    assert_eq!(Some("Renamed"), ctx[user].name.as_deref());
}

#[test]
fn test_has_many_is_a_scoped_relation() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_old).unwrap();
    let comments = Post::COMMENTS.of(&ctx, post).order_asc("created_at").load(&mut ctx).unwrap();
    assert_eq!(vec![Some(fx.first)], comments.iter().map(|c| ctx[*c].id).collect::<Vec<_>>());
}

#[test]
fn test_build_points_the_child_back_at_the_same_owner() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_new).unwrap();
    let before = ctx[post].comments_count.unwrap();
    let comment = Comment { user_id: Some(fx.alice), body: Some("Agreed".into()), ..Comment::new_record() };
    let comment = Post::COMMENTS.build(&mut ctx, post, comment).unwrap();
    assert_eq!(Some(fx.published_new), ctx[comment].post_id);
    ctx.save_bang(comment).unwrap();
    // after_create incremented the owner the controller holds, as in Rails.
    assert_eq!(before + 1, ctx[post].comments_count.unwrap());
}

#[test]
fn test_destroying_a_post_destroys_its_comments() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_old).unwrap();
    ctx.destroy_bang(post).unwrap();
    assert_eq!(0, Comment::all().where_eq("post_id", fx.published_old).count(&mut ctx).unwrap());
}

#[test]
fn test_destroying_a_user_cascades() {
    let (mut ctx, fx) = setup();
    let alice = User::find(&mut ctx, fx.alice).unwrap();
    ctx.destroy_bang(alice).unwrap();
    assert_eq!(0, Post::all().where_eq("user_id", fx.alice).count(&mut ctx).unwrap());
    assert_eq!(0, Comment::all().count(&mut ctx).unwrap());
}
