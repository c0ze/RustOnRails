# Open items

Known defects and loose ends in the runtime, kept here until they're fixed. The compiler's list is in [Rutile's open items](https://github.com/c0ze/Rutile/blob/main/docs/open-items.md). Each entry says what goes wrong and how to see it.

## Known defects

None known.

## Rails parity

- **An Integer past 2**64 - 1 or below -2**63 in a Rails-written session** reads as a Float: `serde_json` keeps only an `f64` for a number outside `i64` and `u64`, so it can't be told from a Float. One too large for a finite `f64` fails the parse, and the whole session reads as empty. From 2**63 to 2**64 - 1 it's a TypeError, as the build expects. `serde_json`'s `arbitrary_precision` would keep the digits.

- **Query bounds know only the bigint range.** `where(estimate: "3000000000")` on an `integer` (int4) column fails to bind, a 500; Rails reads the value as out of the column's range and writes `1=0`. `FromValue::bound` would need the column's limit.
- **A huge numeric string for an enum column** (`where(status: "99999999999999999999")`) becomes `status IS NULL`; Rails writes `1=0`.
- **`save!` after a `before_validation` callback aborts** raises `RecordNotSaved`; Rails raises `RecordInvalid` with empty errors, so a `rescue_from RecordInvalid` handler answers there and not here.
- **An unknown enum label is refused when written, not when assigned.** Rails raises `ArgumentError` at assignment; here validations and before-callbacks run first. Their database changes roll back, but anything else they do doesn't.
- **`true` assigned to a float attribute** is a cast error; Rails stores 1.0.
- **`Model::insert`** is a plain INSERT of every column. Rails' `insert` is `insert_all` with `ON CONFLICT DO NOTHING`, writing only the given keys and the timestamps. Rutile doesn't compile calls to it.
- **After Postgres restarts, each worker's first request that touches the database fails**; the worker reconnects for its next request. A request that doesn't touch the database, like a health check, succeeds and leaves the dead connection in place. Rails 7.1+ reconnects and retries an idempotent read, so there that request succeeds.
- **`sslmode=allow` behaves as `prefer`**: TLS first, plaintext if the server has none. libpq's `allow` tries plaintext first and TLS only if the server refuses it.

## Server

- **Bodies in flight are bounded only per connection**: at the defaults, `MAX_CONNECTIONS` × `MAX_BODY_BYTES` is 5 GiB. A shared budget for buffered bodies would bound the total.
- **The body deadline's credit is earned up front.** A client that sends all but the last byte of a 10 MiB body at once has earned `BODY_TIMEOUT` plus 10 MiB / `MIN_RATE` (about 2.9 hours at the defaults) and can then stall, holding the connection and the buffered body that long. A cap on any single stall inside a body would close it.
- **`OPTIONS *` and absolute-form targets** (`GET http://host/path`) are refused with a 400; RFC 9112 asks servers to accept the absolute form.
- **A bare LF after chunk data** is accepted where RFC 9112 wants CRLF.

## On hold

- **Arrays and hashes read as a scalar param.** `Params::value` is an `Error::Type` for an array or a hash, since a `Value` holds scalars only, so `?status[]=todo&status[]=doing` read as `params[:status]` is a 500 where Rails filters with `IN`. Waiting on arrays in `Value`, in both repositories.

## Housekeeping

- `find_each` (`Relation::batches`) keeps every batch's records in the request's record table; the design's arena per batch, dropped after it, isn't built, so a walk over many rows grows memory until the request ends.
- Two clippy warnings: `from_database` in `src/enums.rs` takes `self`, and a collapsible `if` in `src/http/router.rs`.
- `Ctx::into_client` has no callers.
