# RustOnRails

A Rust crate that implements the parts of the Rails API that a real app touches: ActiveRecord-style models and relations, controllers with filters and strong params, routing, JSON rendering. It is the runtime that [Rutile](../Rutile) compiles Rails apps against, and its names follow Rails closely so the generated Rust reads like the Ruby it came from.

**Status:** the record layer and the web layer work (started 2026-09-25): `model!` structs, queries, validations, callbacks, persistence, associations, `as_json`, params, routing, controllers and a threaded HTTP server. `examples/blog` is Rutile's example app compiled by `rutile build`: its `src/` is generated (edit `Rutile/examples/blog` and run `bundle exec rake example:build` there), its tests are hand-written. It passes the app's model tests in Rust and its 17 Rails integration tests over HTTP, and serves 16 to 27 times the requests per second of Rails ([numbers](../Rutile/docs/benchmarks.md)). `examples/tracker` is Rutile's second example, an ordinary Rails 8 API with token auth, `has_many :through` and pagination. It's generated the same way (`bundle exec rake example:build EXAMPLE=tracker`) and passes the app's 24 integration tests.

Tests need Postgres: start the Rutile repo's cluster with `cd ../Rutile && bundle exec rake pg:start`, or point `RUSTONRAILS_TEST_DATABASE_URL` at your own. `tests/support/schema.sql` is the blog's schema; regenerate it with `mise exec conda:postgresql@16.15 -- pg_dump -h localhost -p 54329 -U postgres --schema-only --no-owner --no-privileges --no-comments -T schema_migrations -T ar_internal_metadata blog_test | grep -v '^\\' | grep -v '^--' | cat -s > tests/support/schema.sql`.

Synchronous, like the Ruby it comes from, on the blocking `postgres` crate. Each request gets its own `Ctx` with a record table, which is how Ruby's shared-object semantics survive without a garbage collector. The details are in [docs/design.md](docs/design.md).

## Development

```bash
cargo test --workspace
```

The workspace holds the crate, the two generated example apps (`examples/blog` and `examples/tracker`; run one with `DATABASE_URL=... cargo run --release -p blog`) and `tools/loadgen`, the load generator behind the benchmark.
