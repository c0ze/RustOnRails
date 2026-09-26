# Changelog

RustOnRails and Rutile share version numbers; each minor version is one milestone of [Rutile's roadmap](../Rutile/docs/roadmap.md).

## 0.10.0

- **Sessions:**
  - Rails 8.1's cookie store: `CookieKey` derives the key as Rails does (PBKDF2-SHA256 of `secret_key_base`) and seals and opens AES-256-GCM cookies in Rails' `_rails` envelope, with the cookie's name as the purpose.
  - `Session` loads and sends back the session under the same rules as Rails.
  - `Cookies` parses and sets plain cookies with Rack's escaping.
  - `Router::session_store` and `server::Config::secret_key_base` set it up.
  - Error pages carry no cookies, as in Rails.
- **Jobs:**
  - `jobs::Job::perform_later` pushes Sidekiq 8's Active Job payload.
  - `jobs::work` is a Sidekiq worker: it runs each job in a fresh `Ctx`, uses Sidekiq's retry backoff and its retry and dead sets, and moves retries that are due back onto their queues.
  - Records go by GlobalID. Active Job's `SerializationError`, `DeserializationError` and `UnknownJobClassError` come with Rails' messages (`Error::Raised`).
  - `jobs::redis` is a small RESP client that takes a password or an ACL user, and a db index.
- **Views:**
  - `View` is the output buffer a compiled template writes. It escapes as Action View does, and keeps `content_for` with Rails' presence rules and the layout's `yield`.
  - `html_escape`, `link_to` and `Response::html`.
  - `path_segment` and `ToParam` serve route helpers: each segment is escaped as Journey escapes it, and a nil id gives Rails' `UrlGenerationError`.
- `Value::inspect`; `find`'s `RecordNotFound` message quotes a String id, as Rails' does.
- **Rails' middleware, from the branch's review:**
  - `Router::default_headers`.
  - `Router::public_page`: error pages follow the request's format (`Request::accepts_html`, `wants_json_errors`, `negotiated`).
  - `Vary: Accept`.
  - `Router::force_ssl`: HSTS and secure cookies.
  - `CookieOptions` for the session store, and `Router::cookies_same_site`.
  - CookieOverflow.
  - The server refuses to start without the secret a session store needs.
  - `Response::headers`, and Rack's full reason-phrase table.
- **Worker, from the branch's review:**
  - It catches panics and reopens a closed database connection.
  - Redis gets timeouts and one reconnect.
  - Sidekiq's `retry`, `dead` and `retry_queue`, the dead set's limits, and the `schedule` set.
  - `jobs::float_argument`, `jobs::arity`; `jobs::configure` checks the URL.

## 0.9.0

- No runtime changes: `rutile package` vendors this crate beside the app's, with its `Cargo.lock`, so a release image builds offline. The examples are regenerated.

## 0.8.0

- `Value` does Ruby's operators on a value whose class is known only at run time: `add`, `sub`, `mul`, `div`, `modulo`, `equals`, `compare`, `is_truthy`, `to_s`, `to_f`. Each gives Ruby's result or Ruby's error: `Error::Type` (TypeError), `Error::Argument` (ArgumentError), `Error::ZeroDivision`, `Error::Nil` and `Error::NoMethod`. An Integer and a Float compare exactly.
- `div_integers`, `mod_integers` and `mod_floats` are Ruby's floor division and modulo for typed numbers.
- `Response::json_value` is `render json:` of a Value: a String goes out as it is, as Rails sends it.
- `Params::value` and `Params::fetch` return a `Result`. An array or a hash is an error rather than nil, since a Value holds only scalars.
- Behaviour changes:
  - A Time compares with a Date as the Date's midnight, as Active Support does.
  - Date and Time arithmetic past chrono's range raises "time out of range" instead of panicking.
  - `"ab" * NaN` raises.
  - `-0.0` prints with its sign.

## 0.7.0

- Relations compute `count`, `sum`, `minimum`, `maximum`, `pluck` and `exists?` in the SQL Rails writes: aggregates drop the order and keep the limit and offset; a count with a limit counts a subquery; `limit(0)` needs no query. `numeric` values (the `SUM` of a bigint) read back as Integers.
- `Relation::batches` is `find_each`: by id, after the last id seen, with the relation's limit capping the total.
- `Ctx::transaction_block` and `Request::transaction_block` run app code's `transaction do ... end`: the block's value, or `None` on `Error::Rollback`. A transaction inside an open one joins it, as in Active Record, so `save` inside a block no longer takes a savepoint of its own; the test's own transaction can't be joined, like Rails' fixture transaction.
- `sum_integers` and `sum_floats` are `Array#sum`, raising `Error::NilCoerced` on a nil element.
- A relation keeps the records `load` read: `size`, `is_any`, `first`, `contains`, `pluck` and `batches` answer from them, as a loaded `ActiveRecord::Relation` does; builders start unloaded.
- A rollback puts back the records the transaction touched (id, saved state, destroyed), and `Error::Rollback` from a callback makes `save` false and `save!` return.
- `HasMany::of` an unsaved owner is `Relation::none`; `limit(0).first` is `None`; enum aggregates are integers.

## 0.6.0

- `examples/store`, Rutile's third example, generated with methods that take typed parameters. No runtime changes were needed: typed parameters are plain Rust arguments.

## 0.5.0

The first tagged version: the runtime the 0.5.0 compiler generates code for.

- Records: `model!` structs, the per-request record table, validations, callbacks, dirty tracking, enums, dates, normalization and secure tokens.
- Relations: lazy queries with joins, fragments, pagination, `includes`, and prepared statements cached per connection, as Rails' adapter does.
- Web: params and strong parameters, a router, controllers with filters and `rescue_from`, JSON rendering, and a threaded HTTP/1.1 server.
- `examples/blog` and `examples/tracker`, both generated by Rutile, and `tools/loadgen` for the benchmark.
