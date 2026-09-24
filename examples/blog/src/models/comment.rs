use std::sync::LazyLock;

use rustonrails::{Behavior, BelongsTo, Check, Ctx, Error, Handle, Model, Result, Time, model};

use super::{Post, User};

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
