//! ../Rutile/examples/blog/test/fixtures/*.yml, inserted like Rails
//! fixtures: no validations or callbacks.

use chrono::TimeDelta;
use rustonrails::{Ctx, Model, Record, now};

use super::{Comment, Post, User};

pub struct Fixtures {
    pub alice: i64,
    pub bob: i64,
    pub published_old: i64,
    pub published_new: i64,
    pub draft: i64,
    pub first: i64,
}

pub fn load(ctx: &mut Ctx) -> Fixtures {
    let days_ago = |n: i64| Some(now() - TimeDelta::days(n));
    let user = |ctx: &mut Ctx, name: &str, email: &str| {
        User::insert(ctx, User { name: Some(name.into()), email: Some(email.into()), ..User::new_record() }).unwrap()
    };
    let alice = user(ctx, "Alice", "alice@example.com");
    let bob = user(ctx, "Bob", "bob@example.com");
    let post = |ctx: &mut Ctx, record: Post| Post::insert(ctx, record).unwrap();
    let published_old = post(ctx, Post {
        user_id: Some(alice), title: Some("Old news".into()), body: Some("First post".into()),
        status: Some("published".into()), published_at: days_ago(3), created_at: days_ago(3), comments_count: Some(1),
        ..Post::new_record()
    });
    let published_new = post(ctx, Post {
        user_id: Some(bob), title: Some("Fresh news".into()), body: Some("Second post".into()),
        status: Some("published".into()), published_at: days_ago(1), created_at: days_ago(1),
        ..Post::new_record()
    });
    let draft = post(ctx, Post {
        user_id: Some(alice), title: Some("Work in progress".into()), body: Some("Not yet".into()),
        ..Post::new_record()
    });
    let first = Comment::insert(ctx, Comment {
        post_id: Some(published_old), user_id: Some(bob), body: Some("Nice one".into()), ..Comment::new_record()
    })
    .unwrap();
    Fixtures { alice, bob, published_old, published_new, draft, first }
}
