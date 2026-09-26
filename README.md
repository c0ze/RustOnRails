# RustOnRails

A Rust crate that implements the parts of the Rails API that a real app touches: ActiveRecord-style models and relations, controllers with filters and strong params, routing, JSON rendering. It is the runtime that [Rutile](https://github.com/c0ze/Rutile) compiles Rails apps against, and its names follow Rails closely so the generated Rust reads like the Ruby it came from.

**Status:** the record layer and the web layer work (started 2026-09-25): `model!` structs, queries, validations, callbacks, persistence, associations, `as_json`, params, routing, controllers and a threaded HTTP server. `examples/blog` is Rutile's example app compiled by `rutile build`: its `src/` is generated (edit `examples/blog` in Rutile and run `bundle exec rake example:build` there), its tests are hand-written. It passes the app's model tests in Rust and its 17 Rails integration tests over HTTP, and serves 33 to 55 times the requests per second of one Puma process with YJIT, and 6 to 8 times a Puma cluster on every core ([numbers](https://github.com/c0ze/Rutile/blob/main/docs/benchmarks.md)). `examples/tracker` is Rutile's second example, an ordinary Rails 8 API with token auth, `has_many :through` and pagination. It's generated the same way (`bundle exec rake example:build EXAMPLE=tracker`) and passes the app's 24 integration tests.

Synchronous, like the Ruby it comes from, on the blocking `postgres` crate. Each request gets its own `Ctx` with a record table, which is how Ruby's shared-object semantics survive without a garbage collector. The details are in [docs/design.md](docs/design.md); what's known to be missing or wrong is in [docs/open-items.md](docs/open-items.md).

## Running an app

A crate `rutile build` generates is configured from the environment, like a Rails app under Puma:

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | (required) | A Postgres URL or `key=value` string. `sslmode` and `sslrootcert` work as in libpq: `disable`, `allow` and `prefer` (TLS when the server offers it, unverified; the default), `require` (TLS, unverified unless there's a root file), `verify-ca` and `verify-full`. A root file is `sslrootcert` or `~/.postgresql/root.crt`, and its CAs are the only ones trusted; `sslrootcert=system` trusts the system's, with `verify-full` only. |
| `BIND` | `127.0.0.1:3000` | Address to listen on. |
| `WORKERS` | `5` | Worker threads, each with its own database connection: Puma's threads. |
| `MAX_CONNECTIONS` | `512` | Connections served at once; past it a new one gets a 503. |
| `IDLE_TIMEOUT` | `20` | Seconds a kept-alive connection may wait for its next request. |
| `HEADER_TIMEOUT` | `20` | Seconds from a request's first byte to the end of its headers, then a 408. |
| `BODY_TIMEOUT` | `60` | Seconds for a request body, plus one for each `MIN_RATE` bytes that arrive; past that, a 408. |
| `WRITE_TIMEOUT` | `60` | The same for writing a response. |
| `MIN_RATE` | `1024` | Bytes a second a body or response must average once its timeout is spent, so a large one can take as long as it needs and a trickle can't. |
| `MAX_BODY_BYTES` | `10485760` | The largest request body; a bigger one is a 413. |

A program that starts the server itself passes `rustonrails::Limits` in `server::Config`.

## Development

```bash
cargo test --workspace
```

The workspace holds the crate, the two generated example apps (`examples/blog` and `examples/tracker`; run one with `DATABASE_URL=... cargo run --release -p blog`) and `tools/loadgen`, the load generator behind the benchmark. TLS builds on the system's OpenSSL, through `native-tls`.

Tests need Postgres. With [Rutile](https://github.com/c0ze/Rutile) cloned next to this repository, `bundle exec rake pg:start` there starts the cluster the tests use; or point `RUSTONRAILS_TEST_DATABASE_URL` at your own. The tests load the blog's schema from `examples/blog/db/schema.sql`; regenerate it with `pg_dump --schema-only --no-owner --no-privileges --no-comments -T schema_migrations -T ar_internal_metadata` of the blog's test database, dropping the `\` and `--` lines. Only a database the tests loaded, or an empty one, is ever reset.

`tests/tls_test.rs` checks every `sslmode` against a Postgres that has TLS. It's ignored by default; see the file for how to point it at one.
