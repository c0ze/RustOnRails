# RustOnRails design

Started 2026-09-25. The compiler side (subset, types, pipeline) is in [Rutile's design](https://github.com/c0ze/Rutile/blob/main/docs/design.md).

## Role

Generated code calls `Post::find`, `post.comments`, `params.require(:post).permit(...)`, `render json:`. RustOnRails provides those, with Rails behavior where the app can observe it: callback order, validation error messages, dirty tracking, relation laziness, status codes for `RecordNotFound` and `RecordInvalid`.

Writing RustOnRails apps by hand is possible but not a goal. Loco already covers that and is the reference for mapping Rails ideas to Rust. We don't build on it because generated code needs exact ActiveRecord semantics, and owning a thin layer over the database driver is simpler than bending SeaORM to match them. That is a judgment; revisit it if the record layer turns out bigger than expected.

## Synchronous code

Generated code is synchronous, like the Ruby it comes from. Rails serves concurrent requests with threads (Puma), and so does RustOnRails: each request runs on a worker thread with its own `Ctx`, and the database driver is the blocking `postgres` crate. An async runtime would force `async`/`.await` onto every generated method that might touch the database, which is nearly all of them, and turn callback tables into boxed futures. The HTTP layer can still be async at the edge and hand requests to a thread pool; that decision belongs to the controller plan. (Decided 2026-09-25; this replaces the earlier tokio + sqlx choice.)

## Stack

| Concern | Crate |
|---|---|
| database | postgres (blocking), Postgres first |
| time | chrono (`NaiveDateTime` in UTC, as Rails stores `datetime`; `NaiveDate` for `date`) |
| validations | regex |
| HTTP | our own HTTP/1.1 framing on `std::net` (a thread per connection, a fixed worker pool); routing is our own regex matcher |
| JSON | serde_json with `preserve_order`, so rendered keys keep column order |
| query strings | form_urlencoded |
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

**Batches** (not built yet). `find_each` and `in_batches` will open a nested arena per batch and drop it after, so a job walking ten million rows keeps flat memory. The trade-off is that memory used inside one huge request is only returned when it ends; batching is the answer there too.

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
            Behavior::<Post>::new()
                .belongs_to::<User>("user", "user_id")
                .enumeration("status", &[("draft", 0), ("published", 1)], true)
                .validates("title", Check::Presence)
                .before_save(Post::stamp_published_at).when(|ctx, post| ctx[post].is_published())
        });
        &BEHAVIOR
    }
}
```

A behavior names its model up front (`Behavior::<Post>::new()`), because closures passed to the builder can't infer it otherwise.

Enum attributes hold the label (`"draft"`), and the record layer maps labels to integers going into and out of the database, so an unknown label stays in memory for the inclusion validator to reject, as in Rails. `normalizes` is a `fn(String) -> String` on the model that the Behavior names. It runs after the type cast, both on assignment and on every value a query binds for that column (`where`, `find_by`, the uniqueness check), as Active Model's `NormalizedValueType` does; nil is left alone. `has_secure_token` fills a blank attribute when a record is built, which is Rails 7.1's `on: :initialize`. With `on: :create` it's a before_create hook calling `Ctx::fill_secure_token`. Values assigned from params are also kept as given until the next save, because `numericality` validates that value ("1.5" isn't an integer though the column holds 1). Scopes are extension traits on `Relation<M>`, so `Post.visible.recent` reads `Post::all().visible().recent()`. Class-level methods (`find`, `find_by`, `create!` as `create_bang`) are defaults on the `Model` trait; instance-level ones (`save`, `valid?` as `is_valid`, `update`, `destroy`, `reload`, `increment!`) are methods on `Ctx` that take a handle.

The blog's models in `examples/blog/src/models/`, generated by `rutile build`, show the shape, and `examples/blog/tests/blog_models_test.rs` runs the blog's model tests against them:

```rust
let post = Post::find(ctx, id)?;                                  // Post.find(id)
ctx.update_bang(post, |p| p.status = Some("published".into()))?; // post.update!(status: :published)
let posts = Post::all().visible().recent().load(ctx)?;            // Post.visible.recent
```

One rule for generated code: an argument that reads the `Ctx` has to be evaluated into a local before a call that borrows the `Ctx` mutably. `Post::find(ctx, ctx[comment].post_id)` doesn't compile; `let id = ctx[comment].post_id; Post::find(ctx, id)` does.

## Web layer

Associations are constants on the owner model, so a generated controller reads like the Ruby:

```rust
impl Post {
    pub const USER: BelongsTo<Post, User> = BelongsTo::new("user", "user_id");
    pub const COMMENTS: HasMany<Post, Comment> = HasMany::new("comments", "post_id", Some(&Comment::POST));
}

let posts = Post::all().visible().recent().includes(&Post::USER).limit(20).load(ctx)?;
let comment = Post::COMMENTS.build(ctx, post, Comment::from_attributes(&attributes)?)?;
```

`belongs_to` targets are cached on the owner (per foreign key, so a changed key reloads), `includes` fills that cache with one `IN` query, and `has_many#build` points the child back at the very owner record, which is Rails' automatic `inverse_of`. `dependent: :destroy` is an explicit `before_destroy` calling `destroy_all`, and `dependent: :nullify` one calling `nullify_all`, in declaration order.

A controller is a `Default` struct whose fields are its instance variables. `Controller::before` is the `before_action` chain written out as a match on the action name, `rescue` is `rescue_from`, and `wrap_parameters` gives the wrapper key and attribute names. Routes are built in `routes.rb` order with `action("show", PostsController::show)`, and a constraint that fails falls through to the next route. An error nobody rescues becomes Rails' default status and the exceptions app's `{"status":404,"error":"Not Found"}`.

The server has a fixed pool of worker threads, each owning one Postgres connection and building a fresh `Ctx` per request; a panicking handler costs a 500 and that worker's connection, not the process. A `Connection` keeps the statements prepared on it across requests, up to Rails' `statement_limit` of 1000, so Postgres parses and plans each query once per connection. Relations with a SQL fragment run unprepared, as in Rails, since their binds are written into the SQL. Without the cache, Postgres spent about 3.5 times the CPU per request that it spends for Rails, and the tracker's joined lookup ran at a fifth of its speed now. In front of the pool, each connection gets a thread that reads requests in full (`src/http/wire.rs`): heads up to 16 KiB, bodies up to 10 MiB by Content-Length or chunked, never both, so a slow or lying client holds its own thread and never a worker. `Limits` bounds what those threads can hold: a cap on open connections (a 503 past it), a deadline for a request's headers and one for its body (a 408 past them, however slowly the bytes trickle in), one for writing the response, and an idle timeout between requests; each is configurable from the environment. Each response goes out in one write with `TCP_NODELAY` set. Database connections take TLS as the URL's `sslmode` asks, read as libpq reads it.

This replaced tiny_http, which could be crashed by a single request declaring a huge body (it drained unread bodies with one allocation of the declared size), treated an early disconnect as the end of a body, and held every multi-segment response about 40 ms for the client's delayed ACK. Request handling follows Rails: HEAD runs the matching GET route without the body, a malformed JSON body is a 400 once a route matches, and only the JSON media types are parsed as JSON; form bodies become params.

`examples/blog` and `examples/tracker` are Rutile's two example apps as `rutile build` generates them: models, controllers and `routes.rs` in the manifest's route order, one file per Ruby file. The blog started as a hand port, written the way codegen would, and was replaced by generated code that passes the same tests; the tracker was generated from the start. Rutile's `rake example:verify` (`EXAMPLE=tracker` for the tracker) regenerates one and runs the Rails app's integration tests against it, and `rake example:benchmark` compares either app with Puma using `tools/loadgen`, a small keep-alive load generator in this workspace.

## Planned modules

- `support`: ActiveSupport pieces generated code needs (`blank?`/`present?`, time zones, `1.day.ago`, inflections, a Ruby-compatible `strip`)
- `value`: the dynamic `Value` enum used where Rutile couldn't infer a type (exists; grows with codegen)

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

Generated code returns `Result<_, rustonrails::Error>`. `RecordNotFound` maps to 404 and `RecordInvalid` to 422 by default, as in Rails. `rescue_from` registrations from the manifest override the mapping per controller. Unexpected errors are logged with the request's method and path and return 500; the generated code's `// path:line` comments lead from a Rust backtrace to the Ruby.

## Open questions

- How `Value` and typed code exchange records: handles inside `Value`, probably.
- Transactions and the record table: a failed save restores that record's saved state and id, but not the other records saved or destroyed inside the same transaction. This is a **known defect**, listed in [open-items.md](open-items.md).
- Enums without `validate: true`: Rails raises on an unknown label at assignment; the struct field can't, so the check has to move to save.
