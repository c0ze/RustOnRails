#![allow(dead_code)]
//! examples/blog's models (../Rutile/examples/blog/app/models), written the
//! way `rutile build` should generate them. Comments point at the Ruby.

pub mod fixtures;

use std::sync::LazyLock;

use regex::Regex;
use rustonrails::{Behavior, BelongsTo, Check, HasMany, Ctx, Error, Handle, Model, Relation, Result, Time, model, now};

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

/// URI::MailTo::EMAIL_REGEXP, verbatim.
const EMAIL_REGEXP: &str = r"\A[a-zA-Z0-9.!\#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\z";

impl User {
    // user.rb:2-3
    pub const POSTS: HasMany<User, Post> = HasMany::new("posts", "user_id", Some(&Post::USER));
    pub const COMMENTS: HasMany<User, Comment> = HasMany::new("comments", "user_id", Some(&Comment::USER));
}

impl Model for User {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<User>> = LazyLock::new(|| {
            Behavior::<User>::new()
                // user.rb:2  has_many :posts, dependent: :destroy
                .before_destroy(|ctx, user| User::POSTS.destroy_all(ctx, user))
                // user.rb:3  has_many :comments, dependent: :destroy
                .before_destroy(|ctx, user| User::COMMENTS.destroy_all(ctx, user))
                // user.rb:5  before_validation { self.email = email.to_s.strip.downcase }
                .before_validation(|ctx, user| {
                    let email = ctx[user].email.clone().unwrap_or_default();
                    ctx[user].email = Some(email.trim().to_lowercase());
                    Ok(())
                })
                // user.rb:7
                .validates("name", Check::Presence)
                // user.rb:8
                .validates("email", Check::Presence)
                .validates("email", Check::Uniqueness)
                .validates("email", Check::Format(Regex::new(EMAIL_REGEXP).expect("EMAIL_REGEXP compiles")))
        });
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
    // post.rb:2
    pub const USER: BelongsTo<Post, User> = BelongsTo::new("user", "user_id");
    // post.rb:3
    pub const COMMENTS: HasMany<Post, Comment> = HasMany::new("comments", "post_id", Some(&Comment::POST));

    // post.rb:5  enum :status gives published? and draft?
    pub fn is_published(&self) -> bool {
        self.status.as_deref() == Some("published")
    }

    pub fn is_draft(&self) -> bool {
        self.status.as_deref() == Some("draft")
    }

    // post.rb:16
    fn stamp_published_at(ctx: &mut Ctx, post: Handle<Post>) -> Result<()> {
        let post = &mut ctx[post];
        if post.published_at.is_none() {
            post.published_at = Some(now());
        }
        Ok(())
    }
}

impl Model for Post {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Post>> = LazyLock::new(|| {
            Behavior::<Post>::new()
                // post.rb:2
                .belongs_to(&Post::USER)
                // post.rb:3  has_many :comments, dependent: :destroy
                .before_destroy(|ctx, post| Post::COMMENTS.destroy_all(ctx, post))
                // post.rb:5
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
                // post.rb:7
                .validates("title", Check::Presence)
                .validates("title", Check::Length { minimum: None, maximum: Some(200) })
                // post.rb:12
                .before_save(Post::stamp_published_at)
                .when(|ctx, post| ctx[post].is_published())
        });
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

impl Comment {
    // comment.rb:2-3
    pub const POST: BelongsTo<Comment, Post> = BelongsTo::new("post", "post_id");
    pub const USER: BelongsTo<Comment, User> = BelongsTo::new("user", "user_id");

    // comment.rb:12
    fn post_is_published(ctx: &mut Ctx, comment: Handle<Comment>) -> Result<()> {
        if let Some(post) = Comment::POST.get(ctx, comment)? {
            if ctx[post].is_draft() {
                ctx.errors_mut(comment).add("post", "must be published");
            }
        }
        Ok(())
    }

    // comment.rb:16  post.increment!(:comments_count)
    fn bump_post_counter(ctx: &mut Ctx, comment: Handle<Comment>) -> Result<()> {
        let post = Comment::POST.get(ctx, comment)?.ok_or(Error::Nil { what: "increment!" })?;
        ctx.increment_bang(post, "comments_count", 1)
    }
}

impl Model for Comment {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Comment>> = LazyLock::new(|| {
            Behavior::<Comment>::new()
                // comment.rb:2-3
                .belongs_to(&Comment::POST)
                .belongs_to(&Comment::USER)
                // comment.rb:5
                .validates("body", Check::Presence)
                .validates("body", Check::Length { minimum: None, maximum: Some(2000) })
                // comment.rb:6
                .validate(Comment::post_is_published)
                // comment.rb:8
                .after_create(Comment::bump_post_counter)
        });
        &BEHAVIOR
    }
}
