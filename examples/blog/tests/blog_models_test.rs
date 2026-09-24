//! ../Rutile/examples/blog/test/models/*_test.rb, test for test.

mod fixtures;
mod support;

use fixtures::Fixtures;
use blog::models::{ApplicationRecordScopes, Comment, Post, PostScopes, User};
use chrono::TimeDelta;
use rustonrails::{Ctx, Model, Record, now};

fn setup() -> (Ctx, Fixtures) {
    let mut ctx = support::ctx();
    let fx = fixtures::load(&mut ctx);
    (ctx, fx)
}

// user_test.rb
#[test]
fn user_requires_name_and_email() {
    let (mut ctx, _) = setup();
    let user = ctx.build(User::new_record());
    assert!(!ctx.is_valid(user).unwrap());
    assert!(ctx.errors(user).on("name").contains(&"can't be blank"));
    assert!(ctx.errors(user).on("email").contains(&"can't be blank"));
}

#[test]
fn user_normalizes_email_before_validation() {
    let (mut ctx, _) = setup();
    let user = User::create_bang(&mut ctx, User { name: Some("Carol".into()), email: Some("  Carol@Example.COM ".into()), ..User::new_record() }).unwrap();
    assert_eq!(Some("carol@example.com"), ctx[user].email.as_deref());
}

#[test]
fn user_rejects_a_malformed_email() {
    let (mut ctx, _) = setup();
    let user = ctx.build(User { name: Some("Dan".into()), email: Some("not-an-email".into()), ..User::new_record() });
    assert!(!ctx.is_valid(user).unwrap());
    assert!(ctx.errors(user).on("email").contains(&"is invalid"));
}

#[test]
fn user_email_is_unique_after_normalizing() {
    let (mut ctx, _) = setup();
    let user = ctx.build(User { name: Some("Alice 2".into()), email: Some("ALICE@example.com".into()), ..User::new_record() });
    assert!(!ctx.is_valid(user).unwrap());
    assert!(ctx.errors(user).on("email").contains(&"has already been taken"));
}

// post_test.rb
#[test]
fn post_title_is_required_and_at_most_200_characters() {
    let (mut ctx, fx) = setup();
    let post = ctx.build(Post { user_id: Some(fx.alice), title: Some(String::new()), ..Post::new_record() });
    assert!(!ctx.is_valid(post).unwrap());
    assert!(ctx.errors(post).on("title").contains(&"can't be blank"));
    ctx[post].title = Some("x".repeat(201));
    assert!(!ctx.is_valid(post).unwrap());
    assert!(ctx.errors(post).on("title").contains(&"is too long (maximum is 200 characters)"));
}

#[test]
fn post_unknown_status_is_a_validation_error() {
    let (mut ctx, fx) = setup();
    let post = ctx.build(Post { user_id: Some(fx.alice), title: Some("Hi".into()), status: Some("archived".into()), ..Post::new_record() });
    assert!(!ctx.is_valid(post).unwrap());
    assert!(ctx.errors(post).on("status").contains(&"is not included in the list"));
}

#[test]
fn post_publishing_stamps_published_at_once() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.draft).unwrap();
    assert_eq!(None, ctx[post].published_at);
    ctx.update_bang(post, |p| p.status = Some("published".into())).unwrap();
    ctx.reload(post).unwrap();
    let stamped = ctx[post].published_at;
    assert!(stamped.is_some());
    ctx.update_bang(post, |p| p.title = Some("Edited".into())).unwrap();
    ctx.reload(post).unwrap();
    assert_eq!(stamped, ctx[post].published_at);
}

#[test]
fn post_drafts_never_get_published_at() {
    let (mut ctx, fx) = setup();
    let post = Post::create_bang(&mut ctx, Post { user_id: Some(fx.bob), title: Some("Still a draft".into()), ..Post::new_record() }).unwrap();
    assert_eq!(None, ctx[post].published_at);
}

#[test]
fn post_visible_recent_lists_published_posts_newest_first() {
    let (mut ctx, fx) = setup();
    let posts = Post::all().visible().recent().load(&mut ctx).unwrap();
    let ids: Vec<Option<i64>> = posts.iter().map(|p| ctx[*p].id).collect();
    assert_eq!(vec![Some(fx.published_new), Some(fx.published_old)], ids);
}

#[test]
fn post_created_since_filters_by_creation_time() {
    let (mut ctx, fx) = setup();
    let recent = Post::all().created_since(now() - TimeDelta::days(2)).load(&mut ctx).unwrap();
    let ids: Vec<Option<i64>> = recent.iter().map(|p| ctx[*p].id).collect();
    assert!(ids.contains(&Some(fx.published_new)));
    assert!(!ids.contains(&Some(fx.published_old)));
}

// comment_test.rb
#[test]
fn comment_body_is_required() {
    let (mut ctx, fx) = setup();
    let comment = ctx.build(Comment { post_id: Some(fx.published_old), user_id: Some(fx.bob), ..Comment::new_record() });
    assert!(!ctx.is_valid(comment).unwrap());
    assert!(ctx.errors(comment).on("body").contains(&"can't be blank"));
}

#[test]
fn comment_creation_bumps_the_posts_comments_count() {
    let (mut ctx, fx) = setup();
    let post = Post::find(&mut ctx, fx.published_new).unwrap();
    let before = ctx[post].comments_count.unwrap();
    Comment::create_bang(&mut ctx, Comment { post_id: Some(fx.published_new), user_id: Some(fx.alice), body: Some("Agreed".into()), ..Comment::new_record() }).unwrap();
    ctx.reload(post).unwrap();
    assert_eq!(before + 1, ctx[post].comments_count.unwrap());
}

#[test]
fn comment_posts_must_be_published_before_they_take_comments() {
    let (mut ctx, fx) = setup();
    let comment = ctx.build(Comment { post_id: Some(fx.draft), user_id: Some(fx.bob), body: Some("Early".into()), ..Comment::new_record() });
    assert!(!ctx.is_valid(comment).unwrap());
    assert!(ctx.errors(comment).on("post").contains(&"must be published"));
}
