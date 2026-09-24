#![allow(dead_code)]
//! examples/blog's models (../Rutile/examples/blog/app/models), written the
//! way `rutile build` should generate them. Comments point at the Ruby.

use std::sync::LazyLock;

use rustonrails::{Behavior, Model, Relation, Time, model};

// application_record.rb:4  scope :created_since, ->(time) { where(created_at: time..) }
pub trait ApplicationRecordScopes {
    fn created_since(self, time: Time) -> Self;
}

impl<M: Model> ApplicationRecordScopes for Relation<M> {
    fn created_since(self, time: Time) -> Self {
        self.where_gte("created_at", time)
    }
}

// user.rb
model! {
    pub struct User in "users" {
        id: i64,
        name: String,
        email: String,
        created_at: Time,
        updated_at: Time,
    }
}

impl Model for User {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<User>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}

// post.rb
model! {
    pub struct Post in "posts" {
        id: i64,
        user_id: i64,
        title: String,
        body: String,
        status: String = "draft",
        published_at: Time,
        comments_count: i64 = 0,
        created_at: Time,
        updated_at: Time,
    }
}

impl Post {
    // post.rb:5  enum :status gives published? and draft?
    pub fn is_published(&self) -> bool {
        self.status.as_deref() == Some("published")
    }

    pub fn is_draft(&self) -> bool {
        self.status.as_deref() == Some("draft")
    }
}

impl Model for Post {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Post>> =
            LazyLock::new(|| Behavior::new().enumeration("status", &[("draft", 0), ("published", 1)], true));
        &BEHAVIOR
    }
}

pub trait PostScopes {
    fn recent(self) -> Self;
    fn visible(self) -> Self;
}

impl PostScopes for Relation<Post> {
    // post.rb:9  scope :recent, -> { order(created_at: :desc) }
    fn recent(self) -> Self {
        self.order_desc("created_at")
    }

    // post.rb:10  scope :visible, -> { where(status: :published) }
    fn visible(self) -> Self {
        self.where_eq("status", "published")
    }
}

// comment.rb
model! {
    pub struct Comment in "comments" {
        id: i64,
        post_id: i64,
        user_id: i64,
        body: String,
        created_at: Time,
        updated_at: Time,
    }
}

impl Model for Comment {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Comment>> = LazyLock::new(Behavior::new);
        &BEHAVIOR
    }
}
