# Changelog

RustOnRails and Rutile share version numbers; each minor version is one milestone of [Rutile's roadmap](https://github.com/c0ze/Rutile/blob/main/docs/roadmap.md).

## Unreleased

- The server's limits, each configurable (`Limits`, or the environment): a connection cap, deadlines for a request's headers, its body and the response (the last two growing with the bytes that move, at `MIN_RATE`), and the body size. A client trickling bytes can no longer hold a thread forever, and a slow but steady one isn't cut off. Each connection holds one file descriptor.
- Postgres over TLS, with every libpq `sslmode` and `sslrootcert`.
- HTTP: control bytes in the target or a header, `Transfer-Encoding` on HTTP/1.0 and lowercase methods are refused; running out of threads no longer ends the accept loop; a panic outside a transaction keeps the worker's connection.
- Connections: one ended by a FATAL error is replaced after that request, not the next; statements a migration invalidated are prepared again.
- Records and queries, as Rails does them: an integer past a bigint fails the write instead of saving 0 and is unboundable in queries (`find` of it is a 404); a `where` value that casts to nil matches nothing; `where(x: [a, nil])` and one-element lists; enum writes take labels only and a blank string is nil; `"1_000"` casts to 1000; a new owner's `has_many` is empty; saving a destroyed record is false; `Ctx::same_record` for `==`.
- Datetime strings without seconds or with a `UTC` suffix parse, and one in an unknown format is an error rather than a silent nil.
- `as_json(include:)` leaves out a nil `belongs_to`; `full_messages` of a `:base` error is the message alone; a JSON body that isn't an object is `params[:_json]`.
- Test setup refuses to wipe a database it didn't load.

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
