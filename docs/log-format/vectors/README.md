# Log format test vectors

A worked example of `docs/log-format.md`, format version 1: a lake named
`vectors`, eight events in two segments, as a file-based engine stores
them.

| Position | Kind | What it shows |
|---|---|---|
| 0 | `lake_created` | the first event: `prev` is 64 zeros, `fence` is 0 |
| 1 | `lease_taken` | the new writer's first event carries its own fence |
| 2 | `declared` | a type declaration with its defaults written out, and its schema hash |
| 3 | `declared` | a type with a two-field key |
| 4 | `record_ingested` | a content-derived ingest id (version before: `0`), `source` and `seen_at` |
| 5 | `record_upserted` | an authored id from an idempotency key; a number written `1e+21`; non-ASCII text |
| 6 | `record_upserted` | a key made from two key fields, with `:` percent-encoded inside them |
| 7 | `checkpoint` | the first event of the second segment, listing the sealed first one |

The files:

- `log/00000000000000000000.jsonl`, `log/00000000000000000007.jsonl`: the
  two segments, one canonical event per line.
- `expected.json`: every event's hash and id and how the id was made
  (`random`, `ingest` or `authored`), the declared types' schema hashes,
  and each segment's digest.
- `schema-hash.json`: a view type whose conditions are written as text,
  the shape object it hashes (conditions in typed form, Appendix B), the
  shape's canonical bytes, and the schema hash.

Two implementations check them, sharing no code:

- `crates/lakelands-core/tests/log_vectors.rs` recomputes everything with
  Lakelands' canonical module and schema hash, and reads every event
  into `lakelands_api::event::Event`. It runs in CI with the rest of the
  tests.
- `verify.py` recomputes the same values with its own RFC 8785
  serialiser and Python's standard library: `python3 verify.py`.

The random ids at positions 0, 1, 2, 3, 6 and 7 are fixed values, so the
hashes are reproducible. A change to the format that changes these bytes
changes this directory in the same pull request.
