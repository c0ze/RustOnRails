# Open items

Known defects and loose ends in the runtime, kept here until they're fixed. The compiler's list is in [Rutile's open items](https://github.com/c0ze/Rutile/blob/main/docs/open-items.md). Each entry says what goes wrong and how to see it.

## Known defects

- **A rolled-back transaction restores only the record it saved.** `save` and `destroy` run in a transaction and restore their own record's id, saved state and destroyed flag when it rolls back, but records saved or destroyed inside it (a child in `after_create`, the children of `dependent: :destroy`) keep their new state in memory. Afterwards such a record can say it's persisted with an id whose row was rolled back, and a later `save` of it updates nothing and returns true. Rails' `rolledback!` restores every record the transaction touched. Fixed on the `feature/tooling` branch (each transaction remembers the records it touches); it lands on `main` with that merge.

## Rails parity

- **Query bounds know only the bigint range.** `where(estimate: "3000000000")` on an `integer` (int4) column fails to bind, a 500; Rails reads the value as out of the column's range and writes `1=0`. `FromValue::bound` would need the column's limit.
- **A huge numeric string for an enum column** (`where(status: "99999999999999999999")`) becomes `status IS NULL`; Rails writes `1=0`.
- **`save!` after a `before_validation` callback aborts** raises `RecordNotSaved`; Rails raises `RecordInvalid` with empty errors, so a `rescue_from RecordInvalid` handler answers there and not here.
- **An unknown enum label is refused when written, not when assigned.** Rails raises `ArgumentError` at assignment; here validations and before-callbacks run first. Their database changes roll back, but anything else they do doesn't.
- **`true` assigned to a float attribute** is a cast error; Rails stores 1.0.
- **`Model::insert`** is a plain INSERT of every column. Rails' `insert` is `insert_all` with `ON CONFLICT DO NOTHING`, writing only the given keys and the timestamps. Rutile doesn't compile calls to it.
- **After Postgres restarts, each worker's first request fails** before the worker reconnects. Rails 7.1+ reconnects and retries an idempotent read, so there that request succeeds.

## Server

- **Bodies in flight are bounded only per connection**: at the defaults, `MAX_CONNECTIONS` × `MAX_BODY_BYTES` is 5 GiB. A shared budget for buffered bodies would bound the total.
- **`OPTIONS *` and absolute-form targets** (`GET http://host/path`) are refused with a 400; RFC 9112 asks servers to accept the absolute form.
- **A bare LF after chunk data** is accepted where RFC 9112 wants CRLF.

## On hold

- **Arrays and hashes read as a scalar param.** `Params::value` is nil for an array or a hash, so `?status[]=todo&status[]=doing` read as `params[:status]` is nil, and code that filters on it when present returns everything, where Rails filters with `IN`. Waiting on real array support in the params and `Value` layer, in both repositories.

## Housekeeping

- `find_each` and `in_batches` are described in the design but not built.
- Two clippy warnings: `from_database` in `src/enums.rs` takes `self`, and a collapsible `if` in `src/http/router.rs`.
- `Ctx::into_client` has no callers.
- `main` and the `feature/tooling` branch (0.6 to 0.9) changed the same code in parallel, including `src/association.rs` (both make a new owner's `has_many` empty, differently), `src/persistence.rs`, `src/ctx.rs`, `src/relation.rs`, the generated examples and the changelog. Merging the two needs the conflicts resolved by hand.
