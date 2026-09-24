# RustOnRails design

Started 2026-09-25. The compiler side (subset, types, pipeline) is in [Rutile/docs/design.md](../../Rutile/docs/design.md).

## Role

Generated code calls `Post::find`, `post.comments`, `params.require(:post).permit(...)`, `render json:`. RustOnRails provides those, with Rails behavior where the app can observe it: callback order, validation error messages, dirty tracking, relation laziness, status codes for `RecordNotFound` and `RecordInvalid`.

Writing RustOnRails apps by hand is possible but not a goal. Loco already covers that and is the reference for mapping Rails ideas to Rust. We don't build on it because generated code needs exact ActiveRecord semantics, and owning a thin layer over sqlx is simpler than bending SeaORM to match them. That is a judgment; revisit it if the record layer turns out bigger than expected.

## Stack

| Concern | Crate |
|---|---|
| async runtime | tokio |
| HTTP, routing, middleware | axum, tower, tower-http |
| database | sqlx, Postgres first |
| JSON | serde, serde_json |
| decimals, time | rust_decimal, jiff or chrono (undecided) |
| ordered hashes | indexmap |

## Memory model

Rust has no GC and Rutile users shouldn't have to think about ownership. The model rests on one observation: almost everything a Rails request allocates dies when the response goes out.

**Request arena.** Each request (and each background job) owns an arena. Strings, arrays and intermediate values built during the request live there and are freed in one step at the end. Reference cycles inside a request cost nothing, since they're freed together.

**Record table and handles.** Loaded records live in a per-request table; variables hold `Handle<Post>`, a `Copy` index into it. Ruby's aliasing works as expected:

```rust
let a = Post::find(req, 1).await?; // Handle<Post>
let b = a;
req[b].title = "x".into();
assert_eq!(req[a].title, "x");       // same record, as in Ruby
```

The table doubles as an identity map, so `Post.find(1)` twice in one request loads once. Handles also sidestep the borrow checker: generated code never holds `&mut Post` across calls, and there's no `RefCell` to panic.

**State that outlives a request** is rare in a well-behaved Rails app and has to be explicit:

- constants: frozen, built once (`LazyLock`)
- `Rails.cache`: values copied in and out of moka or Redis
- class variables and mutable globals: rejected by `rutile check` (they aren't safe under multi-threaded Puma either)

**Batches.** `find_each` and `in_batches` open a nested arena per batch and drop it after, so a job walking ten million rows keeps flat memory. The trade-off is that memory used inside one huge request is only returned when it ends; batching is the answer there too.

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

- Whether the arena is a real bump allocator (bumpalo) or just scoped ownership of `Vec`s and `String`s dropped together. Bumpalo doesn't run destructors, which matters for anything holding a connection or file.
- How `Value` and typed code exchange records: handles inside `Value`, probably.
- Transactions and the record table: rolled-back records need their in-memory state reset the way Rails does it.
