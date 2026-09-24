# RustOnRails design

Started 2026-09-25. The compiler side (subset, types, pipeline) is in [Rutile/docs/design.md](../../Rutile/docs/design.md).

## Role

Generated code calls `Post::find`, `post.comments`, `params.require(:post).permit(...)`, `render json:`. RustOnRails provides those, with Rails behavior where the app can observe it: callback order, validation error messages, dirty tracking, relation laziness, status codes for `RecordNotFound` and `RecordInvalid`.

Writing RustOnRails apps by hand is possible but not a goal. Loco already covers that and is the reference for mapping Rails ideas to Rust. We don't build on it because generated code needs exact ActiveRecord semantics, and owning a thin layer over the database driver is simpler than bending SeaORM to match them. That is a judgment; revisit it if the record layer turns out bigger than expected.

## Synchronous code

Generated code is synchronous, like the Ruby it comes from. Rails serves concurrent requests with threads (Puma), and so does RustOnRails: each request runs on a worker thread with its own `Ctx`, and the database driver is the blocking `postgres` crate. An async runtime would force `async`/`.await` onto every generated method that might touch the database, which is nearly all of them, and turn callback tables into boxed futures. The HTTP layer can still be async at the edge and hand requests to a thread pool; that decision belongs to the controller plan. (Decided 2026-09-25; this replaces the earlier tokio + sqlx choice.)

## Stack

| Concern | Crate |
|---|---|
| database | postgres (blocking), Postgres first |
| time | chrono (`NaiveDateTime` in UTC, as Rails stores `datetime`) |
| validations | regex |
| HTTP, routing | undecided; the controller plan picks it |
| JSON | serde_json, when rendering arrives |
| decimals | rust_decimal, when a decimal column arrives |

## Memory model

Rust has no GC and Rutile users shouldn't have to think about ownership. The model rests on one observation: almost everything a Rails request allocates dies when the response goes out.

**Request arena.** Each request (and each background job) owns an arena. Strings, arrays and intermediate values built during the request live there and are freed in one step at the end. Reference cycles inside a request cost nothing, since they're freed together.

**Record table and handles.** Loaded records live in a per-request table inside the `Ctx`; variables hold `Handle<Post>`, a `Copy` index into it. Ruby's aliasing works as expected:

```rust
let a = Post::find(ctx, 1)?; // Handle<Post>
let b = a;
ctx[b].title = Some("x".into());
assert_eq!(ctx[a].title.as_deref(), Some("x")); // same record, as in Ruby
```

There is no identity map: like Rails, each load makes a new record, so two `Post.find(1)` calls give two independent objects. Handles also sidestep the borrow checker: generated code never holds `&mut Post` across calls, and there's no `RefCell` to panic. The "arena" is simply the `Ctx`: its record table is dropped in one step when the request ends. No bump allocator.

**State that outlives a request** is rare in a well-behaved Rails app and has to be explicit:

- constants: frozen, built once (`LazyLock`)
- `Rails.cache`: values copied in and out of moka or Redis
- class variables and mutable globals: rejected by `rutile check` (they aren't safe under multi-threaded Puma either)

**Batches.** `find_each` and `in_batches` open a nested arena per batch and drop it after, so a job walking ten million rows keeps flat memory. The trade-off is that memory used inside one huge request is only returned when it ends; batching is the answer there too.

## Records

A model is a plain struct with one `Option<T>` field per column, because any Ruby attribute can be nil until the database says otherwise; `model!` declares it along with column defaults. Class-level declarations (`belongs_to`, `enum`, `validates`, callbacks) become a static `Behavior` built in source order, since order is behavior: validations and callbacks run in the order they were declared, before and after callbacks alike (checked against Rails 8.1.4).

```rust
rustonrails::model! {
    pub struct Post in "posts" {
        id: i64, user_id: i64, title: String, status: String = "draft", published_at: Time, ...
    }
}

impl Model for Post {
    fn behavior() -> &'static Behavior<Self> {
        static BEHAVIOR: LazyLock<Behavior<Post>> = LazyLock::new(|| {
            Behavior::new()
                .belongs_to::<User>("user", "user_id")
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
                .validates("title", Check::Presence)
                .before_save(Post::stamp_published_at).when(|ctx, post| ctx[post].is_published())
        });
        &BEHAVIOR
    }
}
```

Enum attributes hold the label (`"draft"`), and the record layer maps labels to integers going into and out of the database, so an unknown label stays in memory for the inclusion validator to reject, as in Rails. Scopes are extension traits on `Relation<M>`, so `Post.visible.recent` reads `Post::all().visible().recent()`. Class-level methods (`find`, `find_by`, `create!` as `create_bang`) are defaults on the `Model` trait; instance-level ones (`save`, `valid?` as `is_valid`, `update`, `destroy`, `reload`, `increment!`) are methods on `Ctx` that take a handle.

## Planned modules

- `record`: model trait, attributes, dirty tracking, validations, callbacks, `Relation<T>` as a lazy query builder, associations, the per-request record table
- `controller`: filter chains, strong params, `render`, `rescue_from`, `head`
- `routing`: routes from Rutile's manifest mounted onto axum
- `value`: the dynamic `Value` enum used where Rutile couldn't infer a type
- `support`: ActiveSupport pieces generated code needs (`blank?`/`present?`, time zones, `1.day.ago`, inflections)

The PoC needs `record`, `controller` and `routing`, plus enough of `support` for the test app.

## Gem adapters

Each supported gem gets a module that exposes its Ruby API on top of a Rust crate:

| Gem | Crate | Effort |
|---|---|---|
| bcrypt, jwt | bcrypt / argon2, jsonwebtoken | thin wrapper |
| faraday, httparty | reqwest | thin wrapper |
| redis, `Rails.cache` | redis or fred, moka | thin wrapper |
| aws-sdk, stripe | aws-sdk-rust, async-stripe | thin wrapper |
| ActionMailer | lettre | thin wrapper plus templates |
| jbuilder, serializers | serde_json | the compiler does most of it |
| rack-cors, rack-attack | tower-http, tower-governor | config mapping |
| Sidekiq | rusty-sidekiq | medium; wire-compatible, so Ruby and Rust workers can share a queue |
| Kaminari, Pagy | none needed | LIMIT/OFFSET in `Relation` |
| AASM | none needed | enum plus generated transitions |
| friendly_id | none needed | slug column plus finder |
| ActiveStorage | object_store or opendal | medium |
| ActionCable | axum websockets | medium |
| Devise | axum-login covers sessions only | large; a cut-down Devise with the common modules |

Pundit needs no adapter beyond `authorize`, since Rutile transpiles the policies. ActiveAdmin, rails_admin, paper_trail and ransack are out; apps that use them keep those routes on a Rails sidecar.

## Errors

Generated code returns `Result<_, rustonrails::Error>`. `RecordNotFound` maps to 404 and `RecordInvalid` to 422 by default, as in Rails. `rescue_from` registrations from the manifest override the mapping per controller. Unexpected errors log with the Ruby source location from the generated code's `// path:line` comments and return 500.

## Open questions

- How `Value` and typed code exchange records: handles inside `Value`, probably.
- Transactions and the record table: a failed save restores the record's saved state and id, but Rails also restores `new_record?` and attribute changes in more cases (rollback after a later statement in the same transaction). Revisit when transactions are exposed to app code.
- Association caching: `comment.post` loads the post on every call; Rails caches it on the owner. Matters only when code mutates the loaded object and reads it through the association again.
- Enums without `validate: true`: Rails raises on an unknown label at assignment; the struct field can't, so the check has to move to save.
