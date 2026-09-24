# The log format

**Format version 1. Draft.** This is the first complete draft; a review
pass (lakelands#14) follows before `0.1.0`. Until then the version stays
at 1 and changes land here first.

A lake's event log is its truth (`docs/decisions/001-the-log-is-the-truth.md`).
This document specifies the log so that a lake can be read, verified and
replayed without Lakelands: the envelope every event carries, the
canonical bytes, the hash chain, every kind of event, and how the log is
cut into segments and checkpointed. The Rust form of these types is
`lakelands_api::event`; where the two disagree, this document is right.

A change to the format is a change to this document first, with a
version bump, and to the code second.

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
| `writer` | string | always | The principal the event was written as, `<kind>:<id>`: `human:alice@example.com`, `service:importer`, `system:lakelands`. |
| `origin` | string | always | Which authority wrote it: `source`, `authored`, `derived` or `system`. |
| `source` | string | source events | The source a `source` event came from. |
| `seen_at` | string | when known | When the source observed the values (§1.3). |
| `procedure` | object | when one did | `{ "name", "version" }` of the procedure that produced the values: a verb, a function backing, an embedding model. |
| `context` | object | always | The host's context for the write: `{ "origin": "direct" \| "reactive" \| "scheduled", "depth": n }`. |
| `cause` | integer | consequences | The position of the event this one follows from: a verb's write names its `verb_called`, an accepted proposal's write names its `write_accepted`, a fired trigger names the write that fired it. |
| `fence` | integer | always | The fencing token of the writer lease the event was appended under (§4). `0` for the event at position 0. |
| `idempotency_key` | string | when given | The caller's idempotency key of an authored write. |
| `body` | object | always | The kind and its payload (§2). |
| `hash` | string | always | SHA-256 of the event's canonical bytes without `hash`, as 64 lower-case hex digits (§1.2). |

A member marked "when …" is absent when it does not apply, never `null`.

### 1.2 Canonical bytes and the hash chain

An event's **canonical bytes** are its JSON serialisation under the JSON
Canonicalization Scheme, RFC 8785: members sorted by their UTF-16 code
units, no insignificant whitespace, strings in their minimal escaped
form, numbers in the ECMAScript shortest round-trip form. Decimals never
reach that number form: a `decimal` value is a string of digits (§5), so
it survives canonicalisation exactly.

An event's `hash` is SHA-256 over the canonical bytes of the event object
with its **top-level** `hash` member removed. (A `checkpoint` body has a
`hash` member of its own; it stays, and is hashed like any other
payload.) Each event names its predecessor's hash
in `prev`; the event at position 0 names 64 zeros. The chain makes an
edit, a removal or a reordering anywhere in the log detectable: the first
event after the change no longer matches.

The log stores each event as its canonical bytes (with `hash`) followed
by one line feed (`0x0A`): one event per line. A reader verifying the log
recomputes every hash and every `prev` link.

**Who canonicalises.** The store produces every event's canonical bytes
and its hash, and hands the bytes to the engine with the event's
position, id, `prev`, `hash` and `fence` beside them. The engine stores
the bytes unchanged and returns them unchanged; the store parses and
verifies what it reads back. Byte identity is therefore a property the
conformance suite checks through the engine seam, on every engine.

**Integers.** Every integer in an event — `position`, `version`,
`fence`, `depth`, `format`, `through` and the members of a segment — is
at most 2^53 − 1, so that every JSON implementation reads it exactly and
RFC 8785's number form agrees with it.

### 1.3 Time

`known_at`, `seen_at`, and every date-time value in a payload are UTC
instants written in exactly one form, `YYYY-MM-DDTHH:MM:SS.ffffffZ`
(microseconds, always six digits, always `Z`).

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

### 2.1 Lake and lease

| Kind | Payload | Fold |
|---|---|---|
| `lake_created` | `format` (integer) | Always at position 0, with `origin: system`, `fence: 0`. Declares the format version the lake was created with. |
| `lease_taken` | `fence` (integer), `holder` (string) | A writer took the lease with a fencing token greater than every earlier one. Every later event carries this fence until the next `lease_taken`. |

### 2.2 Ontology

| Kind | Payload | Fold |
|---|---|---|
| `declared` | `declaration`, `version` (integer), `schema_hash` (string, types only) | A declaration enters the ontology, or replaces its earlier version, from this position on. `declaration.declare` is `type`, `value_type`, `verb`, `trigger`, `object_set` or `function`, with the declaration's members beside it. A new version here never reshapes stored records. |
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

- `type` is the field's whole `FieldType` object: `kind`, and
  `value_type`, `target` and `dimensions` where present.
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
- Every `Condition` inside `view` is hashed in its **typed** form: text
  is parsed first, so the same view written as text or as a typed
  expression hashes the same.
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
| `record_restricted` | `type`, `key`, `version` | The record is withheld from principals without the `restricted` grant (a retention hold). |
| `record_unrestricted` | `type`, `key`, `version` | The hold is lifted. |

`version` is the record's version after the event: 1 for the first event
that writes a key, one more for every later event that changes it. A
retired key that is written again continues its count.

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

### 2.6 Checkpoints

| Kind | Payload | Fold |
|---|---|---|
| `checkpoint` | `through` (integer), `hash`, `segments` | Written as the first event of a new segment (§4.2). `through` is the last position of the segment just sealed and `hash` its last event's hash; `segments` lists every sealed segment so far, oldest first, each `{ "first", "last", "digest" }`. |

## 3. Event identifiers

An event's `id` is 32 lower-case hex digits: the first 16 bytes of a
SHA-256 digest, or 16 random bytes.

Every string in a preimage below is its UTF-8 bytes. Declared names are
ASCII identifiers and hold no `0x00`; a source id or a key containing
`0x00` is refused when written. So the separators are unambiguous.

- **Ingest** is content-addressed: `SHA-256("lakelands:ingest:v1" ‖ 0x00
  ‖ source ‖ 0x00 ‖ type ‖ 0x00 ‖ key ‖ 0x00 ‖ canonical(patch))`.
  `patch` is the patch exactly as the event stores it: the store has
  already checked that every field in it is a `source` or `shared` field
  (a record carrying any other field is refused whole, never trimmed,
  per `docs/decisions/012-writes-touch-only-their-fields.md`) and has
  normalised every `date_time` value to the one form of §1.3. A record
  delivered twice by the same source with the same values has the same
  id, and the second delivery appends nothing: at-least-once delivery is
  safe without any caller keeping exactly-once state.
- **An authored write with an idempotency key**: `SHA-256("lakelands:authored:v1"
  ‖ 0x00 ‖ writer ‖ 0x00 ‖ idempotency_key)`, where `writer` is the
  envelope's `<kind>:<id>` string. Type and key are left out on purpose:
  one idempotency key from one principal is one write in the whole lake,
  whatever it wrote. A retry answers the first write's outcome and
  appends nothing. This covers every door that takes a key: `upsert` and
  `retire` (the `record_*` event takes the id), `call` (the
  `verb_called` event takes it) and `propose` (the `write_proposed`
  event takes it).
- **Every other event**: 16 random bytes.

The events a store appends as consequences of one write (a verb's
writes, a fired trigger, a cleared dependent) take random ids; they are
never delivered twice, because their cause is not.

## 4. Writers, segments and checkpoints

### 4.1 One writer

A lake has one writer at a time (`docs/decisions/008-one-writer-per-lake.md`).
A process becomes the writer by taking the lease: it appends
`lease_taken` with a fencing token greater than every earlier token, and
every event it appends afterwards carries that token in `fence`. An
engine refuses an append whose fence is not the current lease's, whose
first position is not the log's next, or whose `prev` is not the log's
last hash.

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

A segment's **digest** is SHA-256 over its file's bytes.

Sealing the active segment ends it at its last event and starts a new one
whose first event is a `checkpoint` (§2.6). The checkpoint repeats the
sealed segment's last hash and lists every sealed segment's range and
digest.

### 4.3 Verifying

A reader verifies a log by recomputing each event's hash and checking
each `prev` against the event before it. From a checkpoint, a reader can
verify every later segment without the earlier ones: the checkpoint's
`hash` stands in for the chain before it. The checkpoint's segment list
says which earlier segments must exist and what their bytes hash to.

A missing or altered sealed segment is therefore always detected, at the
first checkpoint that lists it, and reported as a break at that
checkpoint. It is never skipped silently.

## 5. Values in payloads

Every storage kind has one JSON form in a patch
(`docs/decisions/011-storage-kind-and-value-type.md`):

| Kind | JSON form |
|---|---|
| `text` | string |
| `number` | number |
| `decimal` | string of digits, `-?[0-9]+(\.[0-9]+)?`, as written |
| `boolean` | `true` / `false` |
| `date_time` | string, the one form of §1.3 |
| `bytes` | string, standard base64 with padding |
| `json` | any JSON value |
| `reference` | string, the target record's key |
| `vector` | array of numbers |
| `geo` | a GeoJSON geometry object |

`null` in a patch clears the field.

## 6. Versioning

Every event carries `v`. A reader refuses an event whose `v` is greater
than the newest version it knows, with a message that names both
versions; it never guesses. Within one version the format is fixed: a new
kind, a new member or a changed encoding is a new version, specified
here first. A lake's log may mix versions only upward: an event never
carries a lower `v` than the event before it.
