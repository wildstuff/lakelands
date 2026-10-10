# Log format test vectors

A worked example of `docs/log-format.md`: a lake named `vectors`, as a
file-based engine stores it, in the one format version a release reads
and writes. Every line carries that version as `v`, from position 0,
whose `lake_created` says the same number as its `format`
(`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
The first eight events are two segments:

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
- `rekey.json`: the event after position 7, a `migrated` event whose
  `rekeyed` moves key `7` of `account` to `DE%3A01`: its line, which is
  canonical, and its hash.
- `v3.json` to `v14.json`: the lake's next events, each file's `lines`
  following the file before it without a gap, through position 39, with
  their `hashes`. A file is named for the format version that first
  held what it shows; its lines are in the release's format like every
  other.
  - `v3.json`, positions 9 and 10: a `record_ingested` carrying
    `upstream`, whose id leaves the member out, and a `declared` whose
    derived field calls `date_bucket`; and `misplaced`, a
    `record_retired` carrying `upstream`, which a reader refuses for its
    kind.
  - `v4.json`, 11 and 12: a `write_proposed` whose write is `declare`,
    and one whose write is `migrate` with its preflight's report.
  - `v5.json`, 13: a `declared` type carrying `annotations` on the type,
    two fields and a relation, whose schema hash is the same type's
    without them; and `misplaced`, the same event with `cause` inside
    its `context`, which a reader refuses for where the member sits.
  - `v6.json`, 14 and 15: a `declared` type whose relation is derived
    from matching values, whose schema hash is the same type's with the
    relation carried by links, and a `declared` trigger with
    `max_depth`.
  - `v7.json`, 16: a `declared` trigger whose guard is `retired()`.
  - `v8.json`, 17 to 19: a `declared` view over `slip` whose `where`
    reads a parameter, `param('shelf')`; a `declared` verb on the type
    `slip` (`scope: type`); and its `verb_called`, with no `key`.
  - `v9.json`, 20: a `declared` view over `slip` that merges `shelf` on
    the same key.
  - `v10.json`, 21 and 22: a `declared` value type `slip_state`, and a
    `write_proposed` whose write is `migrate_value_type`, the value type
    over fewer states with its preflight's report.
  - `v11.json`, 23 to 30, one line each: a `declared` type whose derived
    field calls `floor`, one whose derived field calls `date_bucket` by
    a width, an object set whose `where` names a set with `in_set`, a
    `migrated` whose migration renames a relation (`relation_renames`),
    a `record_upserted` of a `view_kept.` setting, which is its first
    version and so carries `made`, a `record_upserted` at version 4
    that carries `made`, a record made again, a `write_proposed` whose
    proposed migration's report holds `restated`, and a `declared` type
    whose field declares `initial`.
  - `v12.json`, 31 to 33: a `declared` function that carries
    `with_the_set`, a `value_type_migrated` that renames a value, and a
    proposed `migrate_value_type` that carries `renames`.
  - `v13.json`, 34 to 37: a `write_proposed` upsert that carries
    `pinned`; a `declared` type with a field of the kind `date`, a
    `declared` value type over it, and a `record_upserted` holding a
    date.
  - `v14.json`, 38 and 39: an `output_recorded` that carries its output,
    and a fill of the same call that names it by `output_at`; and
    `misplaced`, a fill carrying both members, which a reader refuses.
- `schema-hash.json`: a view type whose conditions are written as text,
  the shape object it hashes (conditions in typed form, Appendix B), the
  shape's canonical bytes, and the schema hash; and the hash of the same
  view declared with its `key` left out, which is the same hash, since a
  shape's key is the key its records carry.

Two implementations check them, sharing no code:

- `crates/lakelands-core/tests/log_vectors.rs` recomputes everything with
  Lakelands' canonical module and schema hash, and reads every event
  into `lakelands_api::event::Event`. It runs in CI with the rest of the
  tests.
- `verify.py` recomputes the same values with its own RFC 8785
  serialiser and Python's standard library: `python3 verify.py`.

`verify.py <dir>` checks a lake's log instead: an export
(`lakelands export --to <dir>`) or a SQLite lake directory. It reads
only `<dir>/log/` and checks every line is canonical, the positions run
from 0 and name their files in 20 digits, the hash chain, every
checkpoint against the position, the hash and the files before it, and
that every line is in the one format, `FORMAT` in `verify.py`; it prints
the last position and hash, or the position where the files stop being
the log. `crates/lakelands-store/tests/export.rs` runs it in CI over an
export with a checkpoint and a re-key, and over the same export
damaged, and `crates/lakelands-engine-sqlite/tests/lake_directory.rs`
over a SQLite lake directory; both fail when `python3` is missing.
`python3 verify.py .` checks this directory's own two segments the same
way.

**A release's format step.** `python3 verify.py --stamp N` rewrites this
directory as the same lake's log in format N, with `verify.py`'s own
serialiser: every line's `v`, `lake_created`'s `format`, each `prev` and
`hash` along the chain, the checkpoint's hash and digest, the hashes
`expected.json`, `rekey.json` and the `vN.json` files repeat, the
`misplaced` lines, both negative vectors, and `FORMAT` in `verify.py`.
It then checks what it wrote. No id changes: none is made from `v`. The
repository's tests hold the result to `LOG_FORMAT_VERSION` and to the
version `docs/log-format.md` opens with.

**Two negative vectors.** Each holds the first segment above with the
event at position 6 changed and its hash made right, so only one rule
is broken, and every verifier refuses it at position 6.
`negative/known-at-goes-back/log/` gives it a `known_at` before the one
of position 5 (§1.3):
- `python3 verify.py negative/known-at-goes-back`;
- `lakelands_core::export::verify`, in `log_vectors.rs`;
- `Lake::verify`, over the same events handed to an engine, in
  `crates/lakelands-store/tests/lake.rs`.

`negative/another-format/log/` writes it in the format before the
release's, which `--stamp N` makes N - 1:
- `python3 verify.py negative/another-format`;
- `lakelands_core::export::verify`, in `log_vectors.rs`.

The random ids at positions 0, 1, 2, 3, 6 and 7 are fixed values, so the
hashes are reproducible. A change to the format that changes these bytes
changes this directory in the same pull request.
