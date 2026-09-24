use std::sync::LazyLock;

use rustonrails::{Behavior, BelongsTo, Check, Ctx, Handle, HasMany, Model, Relation, Result, Time, model, now};

use super::{Comment, User};

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
