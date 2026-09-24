# The log format

**Format version 1.** Reviewed so that a verifier can be written from
this document alone (lakelands#14). Until `0.1.0` is tagged, a
correction still lands here without a version bump; from then on every
change bumps the version (§6).

A lake's event log is its truth (`docs/decisions/001-the-log-is-the-truth.md`).
This document specifies the log so that a lake can be read, verified and
replayed without Lakelands: the envelope every event carries, the
canonical bytes, the hash chain, every kind of event, and how the log is
cut into segments and checkpointed. The Rust form of these types is
`lakelands_api::event`; where the two disagree, this document is right.

A change to the format is a change to this document first, with a
version bump, and to the code second.

`docs/log-format/vectors/` holds a worked example: a small lake's log in
its stored form, with every hash, both kinds of content-derived id, the
segment digests and a view's schema hash. `lakelands-core` recomputes
them in a test, and `verify.py` beside them recomputes them with no
Lakelands code; an implementation that agrees with both reads this
document the way it is meant.

## 1. Events

An event is one JSON object. Its members are the **envelope**, shared by
every kind, and `body`, which holds the kind and its payload.

### 1.1 The envelope

| Member | Type | Presence | Meaning |
|---|---|---|---|
| `v` | integer | always | The format version the event was written under. `1`. |
| `lake` | string | always | The lake the event belongs to. |
| `position` | integer | always | The event's coordinate: `0` for the lake's first event, then consecutive, with no gaps. |
| `id` | string | always | The event's identifier: 32 lower-case hex digits (§3). |
| `prev` | string | always | The `hash` of the event at `position - 1`; 64 `0` digits at position 0. |
| `known_at` | string | always | The lake's wall time when the event was appended (§1.3). |
| `writer` | string | always | The principal the event was written as, `<kind>:<id>`, where `<kind>` is `human`, `agent`, `service` or `system`: `human:alice@example.com`, `service:importer`, `system:lakelands`. The id holds no `0x00` (§3). |
| `origin` | string | always | Which authority wrote it: `source`, `authored`, `derived` or `system`. |
| `source` | string | source events | The source a `source` event came from. |
| `seen_at` | string | when known | When the source observed the values (§1.3). |
| `procedure` | object | when one did | `{ "name", "version" }`, both strings, of the procedure that produced the values: for a verb, its name and its declaration's `version` in decimal digits; for a function backing, the function's name and its declaration's `version`; for an embedding, the model as the backend names it and the version it reports. |
| `context` | object | always | The host's context for the write: `{ "origin": "direct" \| "reactive" \| "scheduled", "depth": n }`, `depth` an integer ≥ 0. |
| `cause` | integer | consequences | The position of the event this one follows from: a verb's write names its `verb_called`, an accepted proposal's write names its `write_accepted`, a fired trigger names the write that fired it. |
| `fence` | integer | always | The fencing token of the writer lease the event was appended under (§4); `0` for the event at position 0, which is written before any lease exists in the lake. |
| `idempotency_key` | string | when given | The caller's idempotency key of an authored write. |
| `body` | object | always | The kind and its payload (§2). |
| `hash` | string | always | SHA-256 of the event's canonical bytes without `hash`, as 64 lower-case hex digits (§1.2). |

A member marked "when …" is absent when it does not apply, never `null`.

### 1.2 Canonical bytes and the hash chain

An event's **canonical bytes** are its JSON serialisation under the JSON
Canonicalization Scheme, RFC 8785, encoded as UTF-8 without a byte order
mark: members sorted by their UTF-16 code units, no insignificant
whitespace, strings in their minimal escaped form (only `\"`, `\\`, the
short forms `\b \t \n \f \r`, and `\u00xx` in lower-case hex for the
other characters below `0x20`; everything else, `0x7F` and above
included, is written as itself), numbers in the ECMAScript shortest
round-trip form (`1e+21`, `1e-7`, `0.000001`; `-0` is written `0`).
Decimals never reach that number form: a `decimal` value is a string of
digits (§5), so it survives canonicalisation exactly. An object never
holds the same member name twice, and a string never holds a lone
surrogate; a reader refuses either.

An event's `hash` is SHA-256 over the canonical bytes of the event object
with its **top-level** `hash` member removed. (A `checkpoint` body has a
`hash` member of its own; it stays, and is hashed like any other
payload.) Each event names its predecessor's hash
in `prev`; the event at position 0 names 64 zeros. The chain makes an
edit, a removal or a reordering anywhere in the log detectable: the first
event after the change no longer matches.

The log stores each event as its canonical bytes (with `hash`) followed
by one line feed (`0x0A`): one event per line. A reader verifying the log
recomputes every hash and every `prev` link, and checks that every line
is exactly the canonical bytes of the event it parses to: a line that is
valid JSON but not canonical (another member order, whitespace, a
number written `1.0` or `1E21`) is a break like a wrong hash, because
two implementations would hash it differently.

**Who canonicalises.** The store produces every event's canonical bytes
and its hash, and hands the bytes to the engine with the event's
position, id, `prev`, `hash` and `fence` beside them. The engine stores
the bytes unchanged and returns them unchanged; the store parses and
verifies what it reads back. Byte identity is therefore a property the
conformance suite checks through the engine seam, on every engine.

**Numbers.** Every number anywhere in an event is a finite IEEE 754
double, and every integer-valued number, in the envelope or in a
payload, lies within ±(2^53 − 1). That covers `position`, `version`,
`fence`, `depth`, `format`, `through`, the members of a segment, and
every `number`, `json` or `geo` value a patch carries. Within that range
every JSON implementation reads an integer exactly and RFC 8785 writes it
as its plain digits, so no two implementations disagree on its bytes. A
write carrying a larger integer is refused at the door, never rounded; a
value that needs more digits is a `decimal`.

### 1.3 Time

`known_at`, `seen_at`, and every `date_time` value in a payload are UTC
instants written in exactly one form, `YYYY-MM-DDTHH:MM:SS.ffffffZ`
(microseconds, always six digits, always `Z`).

The one form is the only form in the log. The doors accept friendlier
input from callers (a date alone, fewer fractional digits, an offset such
as `+02:00`, a lower-case `t` or `z`, a space for `T`) and normalise it
to the one form before an event is written. A reader refuses an event
whose date-time is in any other form, even if it names a valid instant.

`known_at` never decreases along the log. A writer whose clock reads
earlier than the previous event's `known_at` writes the previous value
again. Only `position` orders events; `known_at` is when the lake learned
something, and it is what the fold materialises per field.

`seen_at` is the source's capture time for values from a source, as the
source reported it. The lake does not check it against anything.

## 2. Kinds

The set of kinds is closed per format version. A reader that meets a
kind, or a member, it does not know refuses the event and says so; it
never skips it. `body.kind` names the kind.

Where a payload holds a declaration or a proposed write, its members and
their values are the ones Appendix A lists. A member that has a default
may be written out or left absent; a reader treats the two alike, and
nothing a reader computes (a schema hash, a fold) depends on which one
the writer chose.

### 2.1 Lake and lease

| Kind | Payload | Fold |
|---|---|---|
| `lake_created` | `format` (integer) | Always at position 0, with `origin: system`, `fence: 0`, and `format` equal to the event's `v`. Declares the format version the lake was created with. |
| `lease_taken` | `fence` (integer), `holder` (string) | A writer took the lease with a fencing token greater than every earlier one. The event is the first the new writer appends: its envelope `fence` already equals its payload `fence`, and every later event carries that fence until the next `lease_taken`. `holder` is free text for the operator. |

### 2.2 Ontology

| Kind | Payload | Fold |
|---|---|---|
| `declared` | `declaration`, `version` (integer), `schema_hash` (string, types only) | A declaration enters the ontology, or replaces its earlier version, from this position on. `declaration.declare` is `type`, `value_type`, `verb`, `trigger`, `object_set` or `function`, with the declaration's members beside it. `version` is 1 for the first declaration of that `declare` and `name`, one more for each later one. A new version here never reshapes stored records. |
| `migrated` | `migration` (`to`, `renames`), `schema_hash`, `version`, `report_id` | A type's shape changes and every stored record of it is converted by the fold, by the lossless conversions the preflight report `report_id` checked. |

The schema hash covers shape only (`docs/decisions/007-schema-hash-covers-shape.md`).
It is the SHA-256, as 64 lower-case hex digits, of the canonical bytes of
this object:

```
{
  "key":       [field names, in declared order],
  "fields":    [{ "name", "type", "required" }, ...],
  "relations": [{ "name", "target", "cardinality", "fields": [...] }, ...],
  "view":      { "source", "where", "group_by", "measures": [{ "name", "expr" }] }
}
```

- A field is exactly `{ "name", "type", "required" }`. `type` is the
  field's whole `FieldType` object: `kind`, and `value_type`, `target`
  and `dimensions` only where the declaration sets them (absent, never
  `null`). `required` is always written, `false` when the declaration
  leaves it out.
- A relation is exactly `{ "name", "target", "cardinality", "fields" }`.
  `cardinality` is always written, `many_to_one` when the declaration
  leaves it out; `fields` is `[]` when the relation has none.
- `fields` and `relations` are sorted by name; a relation's own
  `fields` likewise. Every declared name is an ASCII identifier (a type
  name: identifiers joined by `.`), and on ASCII the order by code point
  and the order by UTF-16 code unit that §1.2 uses for members are the
  same order, so there is one sort in the whole format.
  `key` keeps its declared order, because the order of key fields is part
  of the key.
- `view` is `{ "source", "where", "group_by", "measures" }` with every
  member present: `where` is `null` when absent, `group_by` is `[]` when
  empty. `group_by` and `measures` keep their declared order: a measure
  may read the measures before it, so their order is meaning.
- All four members are always present: an empty `key`, `fields` or
  `relations` is written `[]`, and a type that is not a view has
  `"view": null`.
- Every `Condition` inside `view` is hashed in its **typed** form
  (Appendix B): text is parsed first, so the same view written as text
  or as a typed expression hashes the same. The typed form keeps what the
  text says, not what it means: `1.50` and `1.5` are different literals,
  and `a and (b and c)` is not `a and b and c`.
- Excluded, because they do not change how a record is stored or
  validated: descriptions, labels, `sensitive`, `searchable`, field
  `authority`, `derived`, `depends_on`, `on_input_change`, a relation's
  `via`, `inverse` and description, `valid_from` / `valid_to`,
  `language`, and `field_meta`.

### 2.3 Records

| Kind | Payload | Fold |
|---|---|---|
| `record_upserted` | `type`, `key`, `version`, `patch` | An authored write. `patch` is sparse: each member sets a field, `null` clears it, an absent field is unchanged (`docs/decisions/012-writes-touch-only-their-fields.md`). |
| `record_ingested` | `type`, `key`, `version`, `patch` | A write from a source; `source` is set in the envelope. Only `source` and `shared` fields may appear in the patch. |
| `record_retired` | `type`, `key`, `version` | The record is retired: invisible to reads at or after this position, its history kept. |
| `record_restricted` | `type`, `key`, `version`, `reason` | The record is withheld from principals without the `restricted` grant (a retention hold); `reason` is the host's. Written by the host through the store facade, never by a caller's door. |
| `record_unrestricted` | `type`, `key`, `version` | The hold is lifted. |

`version` is the record's version after the event: 1 for the first event
that writes a key, one more for every later event that changes it. A
retired key that is written again continues its count.

**The key.** `key` is a string, the same for every event of one record.
For a type that declares key fields, it is made from their values in
declared order: each value in its §5 JSON form as a string (`text` and
`reference` as they are, `decimal` as its digits, `number` in the number
form of §1.2, `boolean` as `true` or `false`, `date_time` in the one
form of §1.3), with `%` written `%25` and `:` written `%3A` inside it,
and the components joined by `:`. A key field is of one of those kinds.
The key of a `ledger_entry` keyed by `account` `DE:01` and `booked`
`2026-09-24T00:00:00.000000Z` is
`DE%3A01:2026-09-24T00%3A00%3A00.000000Z`. A writer may give the key,
the key fields, or both; when both are given they must agree, or the
write is refused. A type with no key fields takes the writer's key as it
is. No key holds `0x00` (§3).

For every field a `record_upserted` or `record_ingested` event sets or
clears, the fold records the field's provenance from the envelope
(`docs/decisions/010-provenance-per-field.md`):

| Per-field meta | From |
|---|---|
| `origin` | `origin` |
| `known_at` | `known_at` |
| `seen_at` | `seen_at` |
| `position` | `position` |
| `writer` | `writer` |
| `source` | `source` |
| `procedure_version` | `procedure.version` |

In the same event, the fold recomputes derived fields and applies
`on_input_change: clear` to every field one of whose `depends_on` inputs
the event changed; a field the fold writes carries `origin: derived` and
the event's other provenance. Nothing about this is written to the log:
the rules are deterministic, so a replay reproduces them.

### 2.4 Verbs and proposals

| Kind | Payload | Fold |
|---|---|---|
| `verb_called` | `verb`, `key`, `payload` | A verb ran. Its writes follow as `record_upserted` events with `cause` set to this position and `procedure` naming the verb. |
| `write_proposed` | `proposal`, `write`, `why` | A write was recorded as a proposal instead of being applied (`docs/decisions/013-verbs-run-by-declared-risk.md`). `write.write` is `upsert`, `retire` or `call`, with that write's arguments. |
| `write_accepted` | `proposal` | The proposal was accepted. Its write follows in the same append, with `cause` set to this position and the proposer as `writer`. |
| `write_rejected` | `proposal`, `reason` | The proposal was rejected; nothing else is written. |

### 2.5 Reactions and recorded outputs

| Kind | Payload | Fold |
|---|---|---|
| `trigger_fired` | `trigger`, `type`, `key` | A write made a trigger's condition hold. Appended in the same append as that write, with `cause` naming it. The durable fact that something is due; acting on it is the host's (`docs/decisions/004-the-change-feed-is-the-log.md`). |
| `object_set_changed` | `set`, `type`, `key`, `joined` (boolean) | A write moved a record into (`true`) or out of (`false`) an object set. Same append, same `cause`. |
| `output_recorded` | `seam`, `input` (digest), `output`, `field` (optional `[type, key, field]`) | The output of a seam the store called: a function backing, an embedding backend, a verb executor. A replay reads this output instead of calling the seam (`docs/decisions/002-determinism-is-the-boundary.md`). |

`seam` names the seam and what was called in it: `function:<backing
kind>:<function>`, `embed:<model>` or `verb:<executor>:<verb>`. `input`
is the SHA-256 of the canonical bytes of everything the seam received
except the lake (which the event already names): for a function,
`{ "function", "inputs", "config" }`; for an embedding, `{ "texts" }`;
for a verb, `{ "verb", "record", "payload", "position" }`. Two calls with
the same `seam` and `input` are the same call, and a replay reads the
recorded output for it.

`output` is written in the JSON form of §5. When `field` names the field
the output fills, it is that field's kind's form: an embedding is a
`vector`, an array of numbers, like a vector a caller writes in a patch.
Otherwise it is the JSON value the seam returned, under the number rule
of §1.2.

### 2.6 Checkpoints

| Kind | Payload | Fold |
|---|---|---|
| `checkpoint` | `through` (integer), `hash`, `segments` | Written as the first event of a new segment (§4.2). `through` is the last position of the segment just sealed, so the checkpoint's own position is `through + 1`; `hash` is the hash of the event at `through`, so it equals the checkpoint's `prev`. `segments` lists every sealed segment so far, the one just sealed included, oldest first, each `{ "first", "last", "digest" }`. |

## 3. Event identifiers

An event's `id` is 32 lower-case hex digits: the first 16 bytes of a
SHA-256 digest (the first 32 digits of its lower-case hex form), or,
for an event of neither kind below, any 16 bytes the writer has not
used in the lake before.

Every string in a preimage below is its UTF-8 bytes, and `‖` joins
bytes with nothing between them but the `0x00` shown. Declared names are
ASCII identifiers and hold no `0x00`; a source id, a record key, a
writer or an idempotency key containing `0x00` is refused when written.
So the separators are unambiguous.

- **Ingest** is content-addressed: `SHA-256("lakelands:ingest:v1" ‖ 0x00
  ‖ source ‖ 0x00 ‖ type ‖ 0x00 ‖ key ‖ 0x00 ‖ version_before ‖ 0x00 ‖
  canonical(patch))`. `version_before` is the record's `version` before
  the event, in decimal digits: `0` for a key not yet written, otherwise
  the event's own `version` minus one.
  `patch` is the patch exactly as the event stores it: the store has
  already checked that every field in it is a `source` or `shared` field
  (a record carrying any other field is refused whole, never trimmed,
  per `docs/decisions/012-writes-touch-only-their-fields.md`) and has
  normalised every `date_time` value to the one form of §1.3. The
  envelope's `seen_at` and `writer` are not in the preimage: a record
  delivered twice by the same source with the same values against the
  same state of the record has the same id whoever relayed it and
  whenever the source captured it, and the second delivery appends
  nothing. At-least-once delivery is safe without any caller keeping
  exactly-once state. Because `version_before` is in the preimage, a
  genuine return to earlier values (A, then B, then A again) meets a
  different state and is a new event, never mistaken for a redelivery.
- **An authored write with an idempotency key**: `SHA-256("lakelands:authored:v1"
  ‖ 0x00 ‖ writer ‖ 0x00 ‖ idempotency_key)`, where `writer` is the
  envelope's `<kind>:<id>` string. Type and key are left out on purpose:
  one idempotency key from one principal is one write in the whole lake,
  whatever it wrote. A retry answers the first write's outcome and
  appends nothing. This covers every door that takes a key: `upsert` and
  `retire` (the `record_*` event takes the id), `call` (the
  `verb_called` event takes it) and `propose` (the `write_proposed`
  event takes it).
- **Every other event**: any id not used before in the lake. Lakelands'
  store derives it as `SHA-256("lakelands:event:v1" ‖ 0x00 ‖ lake ‖ 0x00
  ‖ position)`, the position in decimal digits: a position is used once
  in a lake, so the id is unique without a source of randomness. A
  reader never recomputes these ids; it only requires them unique.

The events a store appends as consequences of one write (a verb's
writes, a fired trigger, an accepted proposal's write) take positional
ids; they are never delivered twice, because their cause is not.

**An ingest that changes nothing appends nothing.** Before its id
matters, the store compares an ingest patch with the record as the fold
holds it: a patch whose every field already holds the value it carries
is counted as a duplicate and appends no event, so re-importing an
unchanged file leaves the log as it was. The rule is ingest's alone; an
authored `upsert` that changes nothing is still written as an event.

## 4. Writers, segments and checkpoints

### 4.1 One writer

A lake has one writer at a time (`docs/decisions/008-one-writer-per-lake.md`).
A process becomes the writer by taking the lease: it appends
`lease_taken` with a fencing token greater than every earlier token, and
every event it appends afterwards carries that token in `fence`. An
engine refuses an append that holds an event whose fence is not the
current lease's — except the event at position 0, whose fence is 0 —
whose first position is not the log's next, or whose `prev` is not the
log's last hash. Creating a lake is one append: `lake_created` at
position 0 under fence 0, then the creator's `lease_taken`.

A process that opens a lake read-only never takes the lease and never
appends; any number of them may read while one process writes.

An append is a group of consecutive events written together, all or
none, durably before the writer answers: one write and its consequences,
or one group-commit window of an ingest.

### 4.2 Segments

The log is cut into **segments**, each a run of consecutive events. A
file-based engine keeps each segment as one file of lines (§1.2) named
by its first position in 20 decimal digits:
`log/00000000000000000000.jsonl`, `log/00000000000000183211.jsonl`, and
so on. The last segment is **active** and takes appends; the others are
**sealed** and never change. An engine that keeps events elsewhere (a
table, an object store) keeps each event's canonical bytes unchanged and
can export its segments as exactly these files.

A segment's **digest** is SHA-256 over its file's bytes: its events'
lines in order, each ending in its line feed, and nothing else (no byte
order mark, no blank line). The first segment starts with `lake_created`;
every later one starts with a `checkpoint`.

Sealing the active segment ends it at its last event and starts a new one
whose first event is a `checkpoint` (§2.6). The checkpoint repeats the
sealed segment's last hash and lists every sealed segment's range and
digest.

### 4.3 Verifying

A reader verifies a log by recomputing each event's hash and checking
each `prev` against the event before it. From a checkpoint, a reader can
verify every later segment without the earlier ones: the checkpoint's
`hash` stands in for the chain before it. The checkpoint's segment list
says which earlier segments must exist and what their bytes hash to; a
reader that holds them checks each digest, and that the listed ranges
start at 0 and follow each other with no gap or overlap.

A missing or altered sealed segment is therefore always detected, at the
first checkpoint that lists it, and reported as a break at that
checkpoint. It is never skipped silently.

## 5. Values in payloads

Every storage kind has one JSON form in a patch
(`docs/decisions/011-storage-kind-and-value-type.md`):

| Kind | JSON form |
|---|---|
| `text` | string |
| `number` | number, under the rule of §1.2 |
| `decimal` | string of digits, `-?[0-9]+(\.[0-9]+)?`, as written |
| `boolean` | `true` / `false` |
| `date_time` | string, the one form of §1.3 |
| `bytes` | string, standard base64 (RFC 4648 §4) with padding |
| `json` | any JSON value, under the rule of §1.2 |
| `reference` | string, the target record's key |
| `vector` | array of numbers, exactly as many as the field's `dimensions` |
| `geo` | a GeoJSON geometry object (RFC 7946) |

`null` in a patch clears the field.

A `vector` element is the double its JSON number denotes, written in the
number form of §1.2. A backend that computes in 32-bit floats widens each
element to the double of the same value, never rounding it through a
shorter decimal: `0.1` as a 32-bit float is written
`0.10000000149011612`. So one embedding has one byte form whoever
records it. The log does not use a denser binary form for vectors: one
form per kind keeps a verifier to one codec, and a denser encoding would
be a new format version (§6).

## 6. Versioning

Every event carries `v`. A reader refuses an event whose `v` is greater
than the newest version it knows, with a message that names both
versions; it never guesses. Within one version the format is fixed: a new
kind, a new member or a changed encoding is a new version, specified
here first. A lake's log may mix versions only upward: an event never
carries a lower `v` than the event before it.

## Appendix A. Encodings

Every enumerated value in the log is one of the lower-case strings
below; no other spelling is read.

| Where | Values |
|---|---|
| a field type's `kind` (§5) | `text`, `number`, `decimal`, `boolean`, `date_time`, `bytes`, `json`, `reference`, `vector`, `geo` |
| an event's `origin` | `source`, `authored`, `derived`, `system` |
| `context.origin` | `direct`, `reactive`, `scheduled` |
| the kind in `writer` | `human`, `agent`, `service`, `system` |
| a field's `authority` | `authored`, `source`, `shared`, `derived` |
| a field's `on_input_change` | `keep`, `clear` |
| a relation's `cardinality` | `one_to_one`, `one_to_many`, `many_to_one`, `many_to_many` |
| a verb's `risk` | `low`, `medium`, `high` |
| a verb's `effect` | `write` (with `set`), `external` (with `executor`) |
| `declaration.declare` | `type`, `value_type`, `verb`, `trigger`, `object_set`, `function` |
| a proposal's `write.write` | `upsert`, `retire`, `call` |

A **declaration** is an object whose `declare` names what it declares,
with that declaration's members beside it. Every member not marked
"required" may be absent, and an absent member means its default:

| Declaration | Members (default when absent) |
|---|---|
| `type` | `name` (required), `key` (`[]`), `fields` (`[]`), `relations` (`[]`), `view` (none), `valid_from`, `valid_to`, `language` (none), `field_meta` (`true`), `description`, `label` (none) |
| a field (in `fields`, a relation's `fields`, a verb's `payload`, a function's `inputs`) | `name` and `type` (required), `required` (`false`), `authority` (`authored`), `derived` (none), `depends_on` (`[]`), `on_input_change` (`keep`), `sensitive` (`false`), `searchable` (`false`), `description`, `label` (none) |
| a field's `type` | `kind` (required), `value_type`, `target`, `dimensions` (none) |
| a relation | `name` and `target` (required), `cardinality` (`many_to_one`), `inverse`, `via` (none), `fields` (`[]`), `description` (none) |
| a `view` | `source` and `measures` (required; each measure `{ "name", "expr" }`), `where` (none), `group_by` (`[]`) |
| `value_type` | `name` and `kind` (required), `constraint` (none), `one_of` (`[]`), `format`, `scale`, `description` (none) |
| `verb` | `name`, `on`, `risk` and `effect` (required), `payload` (`[]`), `requires` (none), `description` (none); `effect: write` carries `set`, a list of `{ "field", "value" }`; `effect: external` carries `executor` |
| `trigger` | `name` and `on` (required), `when`, `description` (none) |
| `object_set` | `name`, `of` and `where` (required), `description` (none) |
| `function` | `name` and `backing` (required), `inputs` (`[]`), `expr` (none), `config` (`null`), `description` (none) |

A **condition** (`where`, `derived`, `constraint`, `requires`, `when`, a
measure's `expr`, an assignment's `value`, a function's `expr`) is
either a string in the text form or an object in the typed form of
Appendix B.

`migrated.migration` is `{ "to", "renames" }`: `to` is the type's new
declaration, the members of a `type` declaration without `declare`;
`renames` (default `[]`) is a list of `[from, to]` field-name pairs.

`write_proposed.write` is the proposed write with its arguments:
`upsert` carries `type`, `key`, `patch`; `retire` carries `type`, `key`;
`call` carries `verb`, `key`, `payload`. Each also carries `opts`
(default `{}`), with `expected_version` and `idempotency_key` when given.

## Appendix B. Expressions in typed form

Conditions are hashed in the typed form (§2.2), so an implementation
that computes a schema hash from a view written as text parses the text
exactly as below.

**The text form.**

```
expr    := or
or      := and ('or' and)*
and     := not ('and' not)*
not     := 'not' not | cmp
cmp     := sum (cmpop sum | 'in' '(' list ')' | 'is' 'not'? 'null')?
cmpop   := '=' | '!=' | '<' | '<=' | '>' | '>='
sum     := product (('+' | '-') product)*
product := unary (('*' | '/') unary)*
unary   := '-' unary | atom
atom    := literal | path | call | '(' expr ')'
call    := ident '(' list? ')'
list    := expr (',' expr)*
path    := ident ('.' ident)*
literal := number | 'string' | 'true' | 'false' | 'null' | duration
number  := [0-9]+ ('.' [0-9]+)?
duration:= number ('d' | 'h' | 'm' | 's')
```

Keywords (`and`, `or`, `not`, `in`, `is`, `null`, `true`, `false`) are
lower case and are whole identifiers: `android` is a path. An identifier
is ASCII: a letter or `_`, then letters, digits or `_`. White space
(Unicode `White_Space`) may stand between tokens but not inside a path,
a number or a duration. A string is single-quoted, and a quote inside it
is written twice (`'it''s'`); there are no other escapes. A number
directly followed by a letter or `_` is an error, unless that letter is
a duration unit and ends the token (`30d` is a duration, `30days` an
error).

**The typed form.** Every node is an object with `op`:

| Node | Members |
|---|---|
| `lit` | `value`: a literal, below |
| `path` | `path`: the identifiers, in order |
| `cmp` | `cmp` (`eq`, `ne`, `lt`, `le`, `gt`, `ge` for `=`, `!=`, `<`, `<=`, `>`, `>=`), `lhs`, `rhs` |
| `arith` | `arith` (`add`, `sub`, `mul`, `div`), `lhs`, `rhs` |
| `neg` | `arg` |
| `and`, `or` | `args`: the operands (text always yields two or more) |
| `not` | `arg` |
| `in` | `arg`, `list`: one or more nodes |
| `is_null` | `arg` |
| `call` | `func`, `args` |

A literal is `{ "type": "null" }`, `{ "type": "bool", "value": true }`,
`{ "type": "number", "value": "<digits as written>" }`,
`{ "type": "text", "value": "<the string>" }` or
`{ "type": "duration", "value": <integer microseconds> }`.

`func` is one of `now`, `today`, `known_at`, `seen_at`, `previous`,
`setting`, `changed`, `coalesce`, `if`, `lower`, `upper`, `trim`, `len`,
`concat`, `substr`, `contains`, `replace`, `starts_with`, `matches`,
`abs`, `round`, `count`, `sum`, `min`, `max`, `avg`, spelled in the text
form exactly as here.

**From text to typed form.**

- `+ -` and `* /` associate to the left: `a - b - c` is
  `sub(sub(a, b), c)`.
- A chain of `and` (or of `or`) at one level becomes one node with all
  its operands; one operand alone is that operand, with no node around
  it. Parentheses only group: `a and (b and c)` is an `and` of `a` and an
  `and` of `b` and `c`.
- `x is null` is `is_null(x)`; `x is not null` is `not(is_null(x))`.
- A leading `-` is always `neg`, also before a number: `-1.5` is
  `neg(lit 1.5)`, never a negative literal.
- A number literal keeps its digits exactly as written, trailing zeros
  included.
- A duration is its number times the unit (`d` 86 400 s, `h` 3 600 s,
  `m` 60 s, `s` 1 s) in microseconds, which must come out whole: `1.5h`
  is `5400000000`, and `30d` is `2592000000000`.

`docs/log-format/vectors/schema-hash.json` shows a view whose conditions
are written as text, its shape object with those conditions in typed
form, and the resulting hash.
