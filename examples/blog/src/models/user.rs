use std::sync::LazyLock;

use rustonrails::{Behavior, Check, HasMany, Model, Regex, Time, model};

use super::{Comment, Post};

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
