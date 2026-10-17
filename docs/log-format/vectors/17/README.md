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
- `further.json`: the lake's next events, positions 8 to 59, each
  following the one before it without a gap. Every entry of its `events`
  holds a `line`, which is canonical, the line's `hash`, `about`, a
  sentence on what the line is there to show, and `shows`, a name for
  it. Each verifier holds one check for each name and refuses a name it
  has none for, so a line cannot be added without saying what it shows
  in both. What they show, in the order of the positions:
  - a `migrated` event whose `rekeyed` moves key `7` of `account` to
    `DE%3A01` (8);
  - a `record_ingested` carrying `upstream`, whose id leaves the member
    out, and a `declared` whose derived field calls `date_bucket` (9,
    10);
  - a `write_proposed` whose write is `declare`, and one whose write is
    `migrate` with its preflight's report (11, 12);
  - a `declared` type carrying `annotations`, whose schema hash is the
    same type's without them (13);
  - a `declared` type whose relation is derived from matching values,
    whose schema hash is the same type's with the relation carried by
    links, and a `declared` trigger with `max_depth` (14, 15);
  - a `declared` trigger whose guard is `retired()` (16);
  - a `declared` view whose `where` reads a parameter, a `declared`
    verb on its type, and its `verb_called`, with no `key` (17 to 19);
  - a `declared` view that merges a source on the same key (20);
  - a `declared` value type, and a proposed `migrate_value_type` over
    fewer states (21, 22);
  - a derived field that calls `floor`, one that calls `date_bucket` by
    a width, an object set whose `where` calls `in_set`, a `migrated`
    that renames a relation, a `view_kept.` setting at its first
    version, a record made again, a proposed migration whose report
    holds `restated`, and a field that declares `initial` (23 to 30);
  - a `declared` function that carries `with_the_set`, a
    `value_type_migrated` that renames a value, and a proposed
    `migrate_value_type` that carries `renames` (31 to 33);
  - a `write_proposed` upsert that carries `pinned`; a type with a field
    of the kind `date`, a value type over it, and a record holding a
    date (34 to 37);
  - an `output_recorded` that carries its output, and a fill of the same
    call that names it by `output_at` (38, 39);
  - a `lake_upgraded` from format 16 to 17 (40), its members alone: in a
    whole lake it follows a writer's `lease_taken` after events of the
    format before, which this lake does not hold;
  - a type whose `email` is declared `erasable`, an ingest of `crm` whose
    `sealed` names `email`, sealed under `crm`'s key `1`, and an output
    computed from it, sealed under the same key with an HMAC `input`
    (41 to 43). The key is SHA-256 of `lakelands:vectors:key`, so a
    reader that holds it opens them; `verify.py` holds the members'
    form, and the ingest's id over the patch as stored;
  - a `declared` object set whose `where` reads across an edge, through
    a derived relation, and a `declared` trigger whose `when` reads so
    and asks `exists` (44, 45): forms format 16 holds and 15 does not
    (`docs/decisions/055-a-path-steps-through-a-reference.md`);
  - a type declared with two constraints, one that admits at ingest and
    one that reads `previous` (46);
  - a `source_forgotten` of `crm`, naming its key's version `1` (47):
    a reader that folds the lake reads every value `crm` sealed as erased;
  - a type `parcel` whose `state` only its verbs write, `authority:
    verb`, with an `initial` (48): a form format 17 holds and 16 does
    not (lakelands#1130);
  - a host's setting `vat_rate` declared, a `decimal` of group
    `operator` with its unit and meaning (49), and its record of
    `sys.setting` carrying the `group` the store stamps from that
    declaration (50): a declaration kind and a member format 17 holds and
    16 does not (lakelands#996);
  - a `write_proposed` whose `declare` carries its plan, the records it
    would compute again and those it would leave naming no live record
    (51) (`docs/decisions/062-a-change-plans-before-it-acts.md`);
  - an ingest of `erp` in the delivery `batch-7` its caller named (52),
    its id made without it: a form format 17 holds and 16 does not
    (lakelands#1153);
  - a `declared` function of the backing `http` at `risk: high`, a type
    `alert` that it computes a field of, a write of an `alert`, and the
    `write_proposed` of the call that write made due, as its writer and
    caused by it: the record, the field and the call's digest on that
    record as that write made it, with no inputs, which are the
    record's and may be erased (53 to 56;
    `docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`);
  - the trigger `shelved` withdrawn, a `declared` of `declare: withdraw`
    at version 2 (57): a declaration form format 17 holds and 16 does not
    (lakelands#1219);
  - a `values_removed` taking the delivery `batch-7` back (58), and the
    `record_retired` of `INV-2`, which only it wrote, in the same append
    (59): a kind format 17 holds and 16 does not
    (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).

  Its `misplaced` holds five lines that are not of the lake. Each
  stands at its position after the event before it, in the release's
  format and with its hash made right, and carries one member where it
  never sits, so a reader refuses it for that alone: `upstream` on a
  `record_retired`, `cause` inside `context`, an `output_recorded`
  with both `output` and `output_at`, `sealed` on a `record_retired`,
  and `delivery` on a `record_retired`.
- `schema-hash.json`: a view type whose conditions are written as text,
  the shape object it hashes (conditions in typed form, Appendix B), the
  shape's canonical bytes, and the schema hash; and the hash of the same
  view declared with its `key` left out, which is the same hash, since a
  shape's key is the key its records carry.

Two implementations check them, sharing no code:

- `crates/lakelands-core/tests/log_vectors.rs` recomputes everything with
  Lakelands' canonical module and schema hash, and reads every event
  into `lakelands_api::event::Event`. It runs in CI with the rest of the
  tests, and holds the lines of `further.json` to its own list of what
  they show, in their order, and to the lake's last position.
- `verify.py` recomputes the same values with its own RFC 8785
  serialiser and Python's standard library: `python3 verify.py`, one
  line a check. `log_vectors.rs` runs it so in CI and holds it to its
  exit and to the number of checks it makes; it fails when `python3` is
  missing.

`verify.py <dir>` checks a lake's log instead: an export
(`lakelands export --to <dir>`) or a SQLite lake directory. It reads
only `<dir>/log/` and checks every line is canonical, the positions run
from 0 and name their files in 20 digits, the hash chain, every
checkpoint against the position, the hash and the files before it, and
that every line is in the one format, `FORMAT` in `verify.py`; it prints
the last position and hash, or the position where the files stop being
the log, and exits non-zero there:

- a directory with no `log/`, or with no segment file in it, is
  `FAIL <dir>/log holds segment files`;
- a segment file of no bytes is `FAIL position N: <file> holds an
  event`, N the position the file should begin at;
- a line that holds an integer beyond ±(2^53 − 1) (§1.2), or a number
  that is not finite, is `FAIL position N: the line has no canonical
  form`, with the number; `log_vectors.rs` holds that one.

`crates/lakelands-store/tests/export.rs` runs it in CI over an
export with a checkpoint and a re-key, and over the same export
damaged, and `crates/lakelands-engine-sqlite/tests/lake_directory.rs`
over a SQLite lake directory; both fail when `python3` is missing.
`python3 verify.py .` checks this directory's own two segments the same
way.

**A release's format step.** The step copies this directory to
`../<N>/` and runs `python3 verify.py --stamp N` in the copy; this
directory then stays as its release ships it (`../README.md`). The stamp
rewrites the copy as the same lake's log in format N, with `verify.py`'s own
serialiser: every line's `v`, `lake_created`'s `format`, each `prev` and
`hash` along the chain, the checkpoint's hash and digest, the hashes
`expected.json` and `further.json` repeat, the `misplaced` lines, both
negative vectors, and `FORMAT` in `verify.py`.
It then checks what it wrote. No id changes: none is made from `v`. The
repository's tests hold the result to `LOG_FORMAT_VERSION` and to the
version `docs/log-format.md` opens with. Called without a number, or
with something that is none, `--stamp` says how it is called and changes
nothing.

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
