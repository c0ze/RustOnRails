# RustOnRails

A Rust crate that implements the parts of the Rails API that a real app touches: ActiveRecord-style models and relations, controllers with filters and strong params, routing, JSON rendering, Rails' cookie sessions, Sidekiq-compatible jobs, and ERB views. It is the runtime that [Rutile](https://github.com/c0ze/Rutile) compiles Rails apps against, and its names follow Rails closely so the generated Rust reads like the Ruby it came from.

**Status:** the record layer and the web layer work (started 2026-09-25): `model!` structs, queries, validations, callbacks, persistence, associations, `as_json`, params, routing, controllers and a threaded HTTP server. `examples/blog` is Rutile's example app compiled by `rutile build`: its `src/` is generated (edit `examples/blog` in Rutile and run `bundle exec rake example:build` there), its tests are hand-written. It passes the app's model tests in Rust and its 17 Rails integration tests over HTTP, and, like the other two examples, serves 19 to 32 times the requests per second of one Puma process with YJIT, and 5 to 9 times a Puma cluster on every core, on an Apple M4 (on most endpoints; 2.9 to 18 times the cluster across all of them) ([numbers](https://github.com/c0ze/Rutile/blob/main/docs/benchmarks.md)). `examples/tracker` is Rutile's second example, an ordinary Rails 8 API with token auth, `has_many :through` and pagination. It's generated the same way (`bundle exec rake example:build EXAMPLE=tracker`) and passes the app's 24 integration tests. `examples/store`, the third, grows with each milestone: typed method parameters, aggregates and transactions, the `Value` fallback, a cart in Rails' session cookie, restocking through Sidekiq jobs that Ruby and Rust workers share, and a storefront rendered from ERB; it passes its 33 integration tests.

Synchronous, like the Ruby it comes from, on the blocking `postgres` crate. Each request gets its own `Ctx` with a record table, which is how Ruby's shared-object semantics survive without a garbage collector. The details are in [docs/design.md](docs/design.md); what's known to be missing or wrong is in [docs/open-items.md](docs/open-items.md).

## Running an app

A crate `rutile build` generates is configured from the environment, like a Rails app under Puma:

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | (required) | A Postgres URL or `key=value` string; in a URL, write `@`, `/` and `?` in the user name, password and query as `%40`, `%2F` and `%3F`. `sslmode` takes libpq's six names, and `sslrootcert` works as in libpq: `disable` (no TLS), `prefer` (TLS when the server offers it, unverified; the default), `allow` (the same as `prefer`, where libpq tries without TLS first), `require` (TLS, unverified unless there's a root file), `verify-ca` and `verify-full`. A root file is `sslrootcert` or `~/.postgresql/root.crt`, and its CAs are the only ones trusted; `sslrootcert=system` trusts the system's, with `verify-full` only. |
| `BIND` | `127.0.0.1:3000` | Address to listen on. |
| `WORKERS` | `5` | Worker threads, each with its own database connection: Puma's threads. |
| `MAX_CONNECTIONS` | `512` | Connections served at once; past it a new one gets a 503. |
| `IDLE_TIMEOUT` | `20` | Seconds a kept-alive connection may wait for its next request. |
| `HEADER_TIMEOUT` | `20` | Seconds from a request's first byte to the end of its headers, then a 408. |
| `BODY_TIMEOUT` | `60` | Seconds for a request body, plus one for each `MIN_RATE` bytes that arrive; past that, a 408. |
| `WRITE_TIMEOUT` | `60` | The same for writing a response. |
| `MIN_RATE` | `1024` | Bytes a second a body or response must average once its timeout is spent, so a large one can take as long as it needs and a trickle can't. |
| `MAX_BODY_BYTES` | `10485760` | The largest request body; a bigger one is a 413. |
| `SECRET_KEY_BASE` | (none) | The app's secret, which encrypts the session cookie. An app with a session store won't start without it, as Rails won't. |
| `REDIS_URL` | `redis://localhost:6379/0` | Sidekiq's Redis, for an app with jobs: `perform_later` pushes there. |

An app with jobs also runs as a worker: `APP work` pops its queues (`--once` for one job), with Sidekiq's retries and dead set.

A program that starts the server itself passes `rustonrails::Limits` in `server::Config`.

## Development

```bash
cargo test --workspace
```

The workspace holds the crate, the three generated example apps (`examples/blog`, `examples/tracker` and `examples/store`; run one with `DATABASE_URL=... cargo run --release -p blog`) and `tools/loadgen`, the load generator behind the benchmark. TLS goes through `native-tls`, which uses the platform's library: Security framework on macOS, SChannel on Windows and OpenSSL elsewhere, so a Linux build needs OpenSSL.

Tests need Postgres, and the job tests also need Redis. With [Rutile](https://github.com/c0ze/Rutile) cloned next to this repository, `bundle exec rake pg:start` there starts the cluster the tests use and `bundle exec rake redis:start` the Redis; or point `RUSTONRAILS_TEST_DATABASE_URL` and `RUSTONRAILS_TEST_REDIS_URL` (default `redis://127.0.0.1:54379/15`) at your own. The tests load the blog's schema from `examples/blog/db/schema.sql`; regenerate it with `pg_dump --schema-only --no-owner --no-privileges --no-comments -T schema_migrations -T ar_internal_metadata` of the blog's test database, dropping the `\` and `--` lines. The tests drop and recreate the `public` schema only when it holds the marker table they write (`rustonrails_schema`) or no tables at all; they look at tables only, so a schema with just views, functions or sequences is dropped too.

`tests/tls_test.rs` checks, against a Postgres that has TLS, no `sslmode` (libpq's default, `prefer`), `disable`, `require`, `verify-ca` and `verify-full`, `sslrootcert` as a file and as `system`, and a `key=value` connection string; it doesn't check `allow`. It's ignored by default; see the file for how to point it at one.
