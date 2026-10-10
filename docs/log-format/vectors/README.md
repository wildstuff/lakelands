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
- `v14.json`: version 14's addition after `v13.json`'s position 37: an
  `output_recorded` that carries its output, written in the lake's
  version, and a fill of the same call that names it by `output_at`,
  written as version 14; `negatives`, that fill written as version 13,
  and `misplaced`, a fill carrying both members, each of which a reader
  refuses.
- `v13.json`: version 13's additions after `v12.json`'s position 33: a
  `write_proposed` upsert that carries `pinned`; a `declared` type with a
  field of the kind `date`, a `declared` value type over it, and a
  `record_upserted` holding a date, written in the lake's version though
  it needs none of its own; and `negatives`, the proposal and the two
  declarations written as version 12, which a reader refuses.
- `v12.json`: version 12's additions after `v11.json`'s position 30: a
  `declared` function that carries `with_the_set`, a `value_type_migrated`
  that renames a value, and a proposed `migrate_value_type` that carries
  `renames`; and `negatives`, each line written as version 11, which a
  reader refuses.
- `v11.json`: version 11's additions after `v10.json`'s position 22, one
  line each: a `declared` type whose derived field calls `floor`, one
  whose derived field calls `date_bucket` by a width, an object set
  whose `where` names a set with `in_set`, a `migrated` whose
  migration renames a relation (`relation_renames`), a
  `record_upserted` of a `view_kept.` setting, which is its first
  version and so carries `made`, and a `record_upserted` at version 4
  that carries `made`, a record made again, a `write_proposed` whose
  proposed migration's report holds `restated`, and a `declared` type
  whose field declares `initial`; and `negatives`, each line written as
  version 10, which a reader refuses.
- `v10.json`: two events after `v9.json`'s position 20: a `declared`
  value type `slip_state`, still version 9, and a version-10
  `write_proposed` whose write is `migrate_value_type`, the value type
  over fewer states with its preflight's report; and `negative`, that
  proposal written as version 9, which a reader refuses.
- `v9.json`: one version-9 event after `v8.json`'s last position, 19: a
  `declared` view over `slip` that merges `shelf` on the same key; and
  `negative`, the same event written as version 8, which a reader refuses.
- `v8.json`: three version-8 events after `v7.json`'s position 16: a
  `declared` view over `slip` whose `where` reads a parameter,
  `param('shelf')`; a `declared` verb on the type `slip`
  (`scope: type`); and its `verb_called`, with no `key`. `negative` is
  the first written as version 7, `negative_verb` the second and
  `negative_call` the third, each of which a reader refuses.
- `v7.json`: one version-7 event after `v6.json`'s position 15: a
  `declared` trigger whose guard is `retired()`; and `negative`, the same
  event written as version 6, which a reader refuses.
- `v6.json`: two version-6 events after `v5.json`'s position 13: a
  `declared` type whose relation is derived from matching values, whose
  schema hash is the same type's with the relation carried by links, and
  a `declared` trigger with `max_depth`; and
  `negative`, the same event written as version 5, which a reader
  refuses.
- `v5.json`: one version-5 event after `v4.json`'s position 12: a
  `declared` type carrying `annotations` on the type, two fields and a
  relation, whose schema hash is the same type's without them; and
  `negative`, the same event written as version 4, which a reader
  refuses; and `misplaced`, the same event with `cause` inside its
  `context`, which a reader refuses for where the member sits.
- `v4.json`: two version-4 events after `v3.json`'s position 10: a
  `write_proposed` whose write is `declare`, and one whose write is
  `migrate` with its preflight's report; and `negative`, the first written
  as version 3, which a reader refuses.
- `v3.json`: two version-3 events after `rekey.json`'s: a
  `record_ingested` carrying `upstream`, whose id leaves the member out,
  and a `declared` whose derived field calls `date_bucket`; and
  `negative`, the first written as version 2, which a reader refuses;
  and `misplaced`, a version-3 `record_retired` carrying `upstream`,
  which a reader refuses for its kind.
- `rekey.json`: one version-2 event after position 7, a `migrated`
  event whose `rekeyed` moves key `7` of `account` to `DE%3A01`: its
  line, which is canonical, and its hash.
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
that format versions are 1 to 11 and never go down; it prints the last position and hash, or the position where the
files stop being the log. `crates/lakelands-store/tests/export.rs` runs
it in CI over an export with a checkpoint and a re-key, and over the
same export damaged, and `crates/lakelands-engine-sqlite/tests/lake_directory.rs`
over a SQLite lake directory; both fail when `python3` is missing. `python3 verify.py .` checks this directory's own
two segments the same way.

**A negative vector.** `negative/known-at-goes-back/log/` holds the
first segment above with the event at position 6 given a `known_at`
before the one of position 5, its hash made right, so only that rule is
broken (§1.3). Every verifier refuses it at position 6:
- `python3 verify.py negative/known-at-goes-back`;
- `lakelands_core::export::verify`, in `log_vectors.rs`;
- `Lake::verify`, over the same events handed to an engine, in
  `crates/lakelands-store/tests/lake.rs`.

The random ids at positions 0, 1, 2, 3, 6 and 7 are fixed values, so the
hashes are reproducible. A change to the format that changes these bytes
changes this directory in the same pull request.
