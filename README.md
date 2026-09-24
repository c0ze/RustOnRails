# RustOnRails

A Rust crate that implements the parts of the Rails API that a real app touches: ActiveRecord-style models and relations, controllers with filters and strong params, routing, JSON rendering. It is the runtime that [Rutile](../Rutile) compiles Rails apps against, and its names follow Rails closely so the generated Rust reads like the Ruby it came from.

**Status:** design stage, started 2026-09-25. The crate is empty.

Built on tokio, axum and sqlx (Postgres first). Each request gets its own memory arena and record table, which is how Ruby's shared-object semantics survive without a garbage collector. The details are in [docs/design.md](docs/design.md).

## Development

```bash
cargo test
```
