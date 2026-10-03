# The log format

**Format version 9.** Reviewed so that a verifier can be written from
this document alone (lakelands#14). `0.1.0` is tagged, so every change
to the format bumps the version, and a lake writes the version it had
until an event needs a newer one (§6). Version 2 adds one member,
`migrated.rekeyed`; a lake that never re-keys a type writes version 1
and reads exactly as before. Version 3 adds the envelope member
`upstream` and a call of `date_bucket` in a declaration. Version 4 adds
the proposal of a declaration or a migration. Version 5 adds a
declaration's `annotations`. Version 6 adds a derived relation and a trigger's `max_depth`.
Version 7 adds a call of `retired` in a declaration. Version 8 adds a
view's `params` and a call of `param`, and a verb called on its type.
Version 9 adds a view's `merge`. §6 says
which event needs which version.

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
| `v` | integer | always | The format version the event was written under: `1`, `2` from the append that holds a lake's first re-keying `migrated` event on, `3` from the append that holds its first event carrying `upstream` or declaring a call of `date_bucket` on, `4` from its first proposal of a declaration or a migration on, `5` from its first declaration carrying `annotations` on, `6` from its first declaration of a derived relation on, and so on: a version is raised for the whole append that needs it (§6). |
| `lake` | string | always | The lake the event belongs to. |
| `position` | integer | always | The event's coordinate: `0` for the lake's first event, then consecutive, with no gaps. |
| `id` | string | always | The event's identifier: 32 lower-case hex digits (§3). |
| `prev` | string | always | The `hash` of the event at `position - 1`; 64 `0` digits at position 0. |
| `known_at` | string | always | The lake's wall time when the event was appended (§1.3). |
| `writer` | string | always | The principal the event was written as, `<kind>:<id>`, where `<kind>` is `human`, `agent`, `service` or `system`: `human:alice@example.com`, `service:importer`, `system:lakelands`. The id holds no `0x00` (§3). |
| `origin` | string | always | Which authority wrote it: `source`, `authored`, `derived` or `system`. |
| `source` | string | source events | The source a `source` event came from. |
| `seen_at` | string | when known | When the source observed the values (§1.3). |
| `upstream` | object | when read from another lake | `{ "lake", "position" }`: the lake whose log the values were read from (its lake id) and the position of the event there whose state they are (an integer). Only on `record_ingested`, from version 3 on (§6); a reader refuses it on any other kind, naming the kind. The ingesting principal states it, as it states `seen_at`. |
| `procedure` | object | when one did | `{ "name", "version" }`, both strings, of the procedure that produced the values: for a verb, its name and its declaration's `version` in decimal digits; for a function backing, the function's name and its declaration's `version`; for an embedding, the model as the backend names it and the version it reports. |
| `context` | object | always | The host's context for the write: `{ "origin": "direct" \| "reactive" \| "scheduled", "depth": n }`, `depth` an integer ≥ 0. It never carries `cause`: a reader refuses a `context` that does, naming it. |
| `cause` | integer | consequences | The position of the event this one follows from: a verb's write names its `verb_called`, an accepted proposal's write names its `write_accepted`, a fired trigger names the write that fired it. An event of a host's write that the store sets no cause for names the position the host gave, at or below the head when it was appended (`docs/decisions/021-the-write-context-belongs-to-the-host.md`); any kind may carry it, in every version. |
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
something, and it is what the fold materialises per field. A verifier
refuses a `known_at` below the one before it, naming the event's
position, as it refuses a format version that goes down (§6); every log
a Lakelands store has written keeps the rule, so checking it moves no
format version. The rule is what lets a reader find the position the
log had at an instant by a binary search (`LakeStore::position_at`).

`seen_at` is the source's capture time for values from a source, as the
source reported it. The lake does not check it against anything.

## 2. Kinds

The set of kinds is closed per format version. A reader that meets a
kind, or a member, it does not know refuses the event and says so; it
never skips it. `body.kind` names the kind.

Where a payload holds a declaration or a proposed write, its members and
their values are the ones Appendix A lists. A declaration member it does
not list, at any depth, is refused like any other, naming the member and
where in the declaration it sits. A member that has a default
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
| `migrated` | `migration` (`to`, `renames`), `schema_hash`, `version`, `report_id`, `rekeyed` (version 2; `[]`) | A type's shape changes and every stored record of it is converted by the fold, by the lossless conversions the preflight report `report_id` checked. When `rekeyed` lists keys, the fold also moves those records to their new keys (§2.3). |

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
  may read the measures before it, so their order is meaning. A view
  that declares parameters has a fifth member, `params`, the list of
  `{ "name", "kind" }` in declared order; a view without them has no
  `params` member, so its hash is the one it had before views could take
  parameters. A view that merges sources has a member `merge` likewise,
  the list of `{ "source", "where", "group_by", "measures" }` in declared
  order, each in the form of the view's own members; a view without one
  has no `merge` member.
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
  `authority`, `via`, `inverse` and description, `valid_from` /
  `valid_to`, `language`, and `field_meta`. A relation's own `fields`
  are shape, above; so is its link type's, which has a schema hash of
  its own.

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

**The key.** `key` is a string, the same for every event of one record
until a migration re-keys it.
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

**Re-keying** (version 2). A `migrated` event whose `rekeyed` lists
`{ "type", "from", "to" }` moves each listed record: from that event's
position on, the record is at `to`, with its fields, its `version` and
its per-field provenance unchanged, and `from` is **ended**: a read at or
after the position does not find it, a read before it does, and a later
record whose key is the same string is a new record whose first event is
version 1. Every `reference` value, in any record, that names a listed
`from` of its target type names the `to` from that position on. The list
covers every type the migration re-keys: the migrated type, and every
type whose key fields reference a re-keyed type, link types among them;
it is explicit, so a reader applies it without making keys again.

**Link records.** A relation without `via` is carried by **link
records**: ordinary records of a **link type** the store derives from
the relation, written and read by the record kinds above. No event kind
is added for links, so this is format version 1 unchanged
(`docs/decisions/035-a-link-is-a-record.md`).

- The link type of relation `r` on type `t` is named `sys.link.t.r`.
  It is declared by the store, as a system declaration in the same
  append as the declaration of `t` that holds the relation, and again
  whenever that declaration changes the relation.
- Its key fields are `from` (a `reference` to `t`) and `to` (a
  `reference` to the relation's `target`), both required, so a link's
  key is `<from key>:<to key>` by the key rule above; one relation holds
  one link per pair. Its other fields are the relation's own `fields`,
  with their declared authority; a relation may not name a field of its
  own `from` or `to`.
- Writing a link is a `record_upserted` (or a `record_ingested`, for a
  relation whose `authority` is `source`) of the link type; removing it
  is a `record_retired`. A link written again after its removal
  continues its version, and every field of a link has its provenance
  like any record's.
- The relation's `cardinality` holds over live links: `many_to_one` and
  `one_to_one` allow one live link per `from`, `one_to_many` and
  `one_to_one` one per `to`. A write that would break it is refused as a
  constraint, and nothing is appended.
- A relation keeps its carrier: a declaration that would give a relation
  `via` it did not have, or take its `via` away, is refused, because the
  links would have to move between a field and link records.
- The `sys.` guard refuses a caller's declaration of a link type, and
  the ordinary doors (`upsert`, `retire`) refuse its records as the
  wrong door; links are written through the relation's link doors, and
  by ingest for a source relation.

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
| `upstream` | `upstream` |

In the same event, the fold recomputes derived fields and applies
`on_input_change: clear` to every field one of whose `depends_on` inputs
the event changed; a field the fold writes carries `origin: derived` and
the event's other provenance. Nothing about this is written to the log:
the rules are deterministic, so a replay reproduces them.

### 2.4 Verbs and proposals

| Kind | Payload | Fold |
|---|---|---|
| `verb_called` | `verb`, `key`, `payload` | A verb ran. Its writes follow as `record_upserted` events with `cause` set to this position and `procedure` naming the verb. From version 8 on, a verb declared on its type (`scope: type`) is called with no record, and `key` is left out. |
| `write_proposed` | `proposal`, `write`, `why` | A write was recorded as a proposal instead of being applied (`docs/decisions/013-verbs-run-by-declared-risk.md`). `write.write` is `upsert`, `retire` or `call`, with that write's arguments; from version 4 on also `declare`, a set of declarations, or `migrate`, a migration with its preflight's report (Appendix A). |
| `write_accepted` | `proposal` | The proposal was accepted. Its write follows in the same append, with `cause` set to this position and the proposer as `writer`: for `declare` the `declared` events, for `migrate` the `migrated` event (the link types the store derives keep `system:lakelands` as writer). |
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
for a verb, `{ "verb", "record", "payload", "position" }`, `record`
null for a verb called on its type. Two calls with
the same `seam` and `input` are the same call, and a replay reads the
recorded output for it.

A function's `inputs` hold every input the function declares, by name,
with the record's value of the field of that name, `null` where the
record holds none; `config` is the function's declared configuration,
`null` when it has none. For example, the call

```json
{"function":"geocode","inputs":{"address":"Hauptstraße 1","country":null},"config":{"url":"https://geo.example/lookup"}}
```

has the canonical bytes

```text
{"config":{"url":"https://geo.example/lookup"},"function":"geocode","inputs":{"address":"Hauptstraße 1","country":null}}
```

and `input` is
`a4702edebf8e4f34b65be7b55b963f930ac4e6f13d0b9a8773ca942814bc77aa`.

An embedding's `texts` hold one text: the `from` fields' values in
declared order, joined with one blank line, empty ones left out. The
store may send several texts in one request, but each vector is recorded
on its own, with the digest of its own one-text call; how calls were
batched never reaches the log. For example, `title` "Harbour crane", an
empty `subtitle` and `summary` "A crane on the quay." make the call with
the canonical bytes

```text
{"texts":["Harbour crane\n\nA crane on the quay."]}
```

and `input` is
`6ff3bba9eed9f7899ca52fb42794fb32c315850b3bc9470dab44481773826ed0`.

A seam that failed is recorded with the output `{"error": "<why>"}`,
an object whose one member is `error`, a string. A replay reads it like
any other output: the call is not made again. A request that carried
several texts and failed records a failure for each of them.

The log may hold two outputs for one `seam` + `input`: a settle forced
by a declaration calls again for an input whose recorded output is a
failure (`docs/decisions/022-functions-are-declarations.md`, extended).
The later output holds. The fold, and a projection's recorded outputs,
answer the latest output recorded for a call from its position on, and
the earlier one before it.

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
  envelope's `seen_at`, `upstream` and `writer` are not in the preimage: a record
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
- **An aging transition** (`docs/decisions/036-aging-runs-by-date-once.md`):
  `SHA-256("lakelands:aging:v1" ‖ 0x00 ‖ rule ‖ 0x00 ‖ type ‖ 0x00 ‖ key
  ‖ 0x00 ‖ date)`, where `rule` is the aging rule's name, `type` and
  `key` the record it moves, and `date` the day aged, in the one form of
  §1.3 at midnight. Such an event is a `record_upserted` whose
  `procedure.name` is `aging:<rule>`, so a reader recognises it. The
  store appends nothing for a transition whose id is already in the
  lake: aging a date twice writes it once.
- **Every other event**: any id not used before in the lake. Lakelands'
  store derives it as `SHA-256("lakelands:event:v1" ‖ 0x00 ‖ lake ‖ 0x00
  ‖ position)`, the position in decimal digits: a position is used once
  in a lake, so the id is unique without a source of randomness. A
  reader never recomputes these ids; it only requires them unique.

The aging id is a new derivation for an event kind version 1 already
has; it adds no kind, member or encoding, so the format version stays 1
(§6). A reader that does not recompute it still reads the event.

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
**sealed** and never change. An engine may keep files of its own beside
`log/`, such as an index or its lease; a verifier reads only `log/`. An engine that keeps events elsewhere (a
table, an object store) keeps each event's canonical bytes unchanged and
can export its segments as exactly these files. An export
(`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`)
may write an `export.json` manifest beside `log/`; it is optional, not
part of the format, and never trusted.

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

A `geo` value may be empty, the same way for every geometry type: empty
`coordinates` are the empty geometry of that type, and a
`GeometryCollection` may hold no `geometries`. Emptiness stops at the
top. A part that is present keeps its type's shape: a position has two
or three numbers, a line string at least two positions, and a ring is
closed, with four positions or more.

Base64 is canonical: the pad bits of the last character before padding
are zero, so one byte string has one spelling. `aGk=` is the only form
of the bytes `hi`; `aGl=` decodes to the same bytes in lenient decoders
and is refused.

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

A version is raised for a whole append. The append that holds a lake's
first event needing version N writes every one of its events with `v`
N, and every event after it carries at least N. An event of that append
that needs nothing newer still carries N. Each row below says which
event raises it; "that append" is the append that holds it.

| Version | Adds |
|---|---|
| 1 | Everything above except what later versions add. |
| 2 | `migrated.rekeyed` (§2.2, §2.3). A lake writes version 1 until the append that holds its first re-keying `migrated` event; that append carries `v` 2, as does every event after it; a version-1 event that carries `rekeyed` is refused. A version-1 reader refuses the version-2 events rather than misreading a re-key. |
| 3 | The envelope member `upstream` (§1.1, §2.3) and a call of `date_bucket` in a declaration's expressions (Appendix B). A lake writes the version it had until the append that holds its first event that carries `upstream`, or that declares (in a `declared` or `migrated` event) an expression calling `date_bucket`; that append carries `v` 3, as does every event after it. A version-2 (or 1) event that carries either is refused, naming it. A version-2 reader refuses the version-3 events rather than misreading them. |
| 4 | A proposal of a declaration or a migration: `write_proposed` whose `write` is `declare` or `migrate` (§2.4, Appendix A). A lake writes the version it had until the append that holds its first such proposal; that append carries `v` 4, as does every event after it; a version-3 (or older) event that carries one is refused, naming it. A version-3 reader refuses the version-4 events rather than failing on a write it does not know. |
| 5 | A declaration's `annotations` (Appendix A): on a type, a field or a relation, anywhere in the declaration a `declared` event carries, in a `migrated` event's `to`, or in a proposed `declare` or `migrate`. A lake writes the version it had until the append that holds its first event whose declaration holds a non-empty `annotations`; that append carries `v` 5, as does every event after it; a version-4 (or older) event that carries annotations is refused, naming them. A version-4 reader refuses the version-5 events rather than refusing a member it does not know. An annotation is never part of the schema hash (§2.2). |
| 6 | A derived relation, a relation declaration's `derived`, and a trigger's `max_depth` (Appendix A). A lake writes the version it had until the append that holds its first event whose declaration holds a relation with `derived` or is a trigger with `max_depth` (a `declared` event, a `migrated` event's `to`, a proposed `declare` or `migrate`); that append carries `v` 6, as does every event after it; a version-5 (or older) event that carries one is refused, naming it. A version-5 reader refuses the version-6 events rather than refusing a member it does not know. `derived` is never part of the schema hash (§2.2). Version 6 took both members while it was untagged; a tagged version is closed. |
| 7 | A call of `retired` in a declaration's expressions (Appendix B): in a trigger's `when`, the one place it means something. A lake writes the version it had until the append that holds its first event whose declaration calls `retired` (a `declared` event, a `migrated` event's `to`, a proposed `declare` or `migrate`); that append carries `v` 7, as does every event after it; a version-6 (or older) event that calls it is refused, naming it. A version-6 reader refuses the version-7 events rather than reading a guard it cannot evaluate, which would never fire. |
| 8 | A view's `params` and a call of `param` in a declaration's expressions (Appendix A, Appendix B): a view may declare parameters, which a read binds and its `where` and measures read with `param('name')`. A lake writes the version it had until the append that holds its first event whose declaration is a view with a non-empty `params`, or calls `param` (a `declared` event, a `migrated` event's `to`, a proposed `declare` or `migrate`); that append carries `v` 8, as does every event after it; a version-7 (or older) event that carries either is refused, naming it. A version-7 reader refuses the version-8 events rather than computing a view whose parameters it cannot bind. `params` is part of the schema hash only when it is not empty (§2.2). The same version adds a verb called on its type (`docs/decisions/013-verbs-run-by-declared-risk.md`, extended): a verb declaration's `scope: type` (in a `declared` event or a proposed `declare`); a `verb_called` without `key`; and a proposed `call` without `key`. A lake writes the version it had until the append that holds its first such event; that append carries `v` 8, as does every event after it, and a version-7 (or older) event that carries one is refused, naming it. Version 8 took both while it was untagged. |
| 9 | A view's `merge` (Appendix A): further sources a view merges on its key, each with its own `where`, `group_by` lined up with the view's by position, and aggregate measures the view's measures read by name. A lake writes the version it had until its first event whose declaration is a view with a non-empty `merge` (a `declared` event, a `migrated` event's `to`, a proposed `declare` or `migrate`), which carries `v` 9, as does every event after it; a version-8 (or older) event that carries one is refused, naming it. A version-8 reader refuses the version-9 events rather than computing a view from its main source alone. `merge` is part of the schema hash only when it is not empty (§2.2). |

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
| a verb's `scope` (version 8; default `record`, which is never written) | `record`, `type` |
| `declaration.declare` | `type`, `value_type`, `verb`, `trigger`, `object_set`, `function` |
| a proposal's `write.write` | `upsert`, `retire`, `call` |

A **declaration** is an object whose `declare` names what it declares,
with that declaration's members beside it. Every member not marked
"required" may be absent, and an absent member means its default:

| Declaration | Members (default when absent) |
|---|---|
| `type` | `name` (required), `key` (`[]`), `fields` (`[]`), `relations` (`[]`), `view` (none), `valid_from`, `valid_to`, `language` (none), `field_meta` (`true`), `description`, `label` (none), `annotations` (`{}`, version 5) |
| a field (in `fields`, a relation's `fields`, a verb's `payload`, a function's `inputs`) | `name` and `type` (required), `required` (`false`), `authority` (`authored`), `derived` (none), `depends_on` (`[]`), `on_input_change` (`keep`), `sensitive` (`false`), `searchable` (`false`), `description`, `label` (none), `annotations` (`{}`, version 5) |
| a field's `type` | `kind` (required), `value_type`, `target`, `dimensions` (none) |
| a relation | `name` and `target` (required), `cardinality` (`many_to_one`), `authority` (`authored`; who writes its links, when it has no `via`), `inverse`, `via` (none), `fields` (`[]`), `description` (none), `annotations` (`{}`, version 5), `derived` (none, version 6: `{ "match": [ { "from", "to" } ] }`, a field of the owner and one of the target per pair) |
| a `view` | `source` and `measures` (required; each measure `{ "name", "expr" }`), `where` (none), `group_by` (`[]`), `params` (`[]`, version 8: each `{ "name", "kind" }`, a kind of `text`, `number`, `decimal`, `boolean` or `date_time`), `merge` (`[]`, version 9: each `{ "source", "where", "group_by", "measures" }`; `source` and `measures` required, `where` none, `group_by` `[]`) |
| `value_type` | `name` and `kind` (required), `constraint` (none), `one_of` (`[]`), `format`, `scale`, `description` (none) |
| `verb` | `name`, `on`, `risk` and `effect` (required), `payload` (`[]`), `requires` (none), `description` (none); `effect: write` carries `set`, a list of `{ "field", "value" }`; `effect: external` carries `executor` |
| `trigger` | `name` and `on` (required), `when`, `description` (none), `max_depth` (none, version 6: an integer at most the lake's `max_reactive_depth`) |
| `object_set` | `name`, `of` and `where` (required), `description` (none) |
| `function` | `name` and `backing` (required), `inputs` (`[]`), `expr` (none), `config` (`null`), `description` (none) |

An **annotation map** (`annotations`, version 5) is an object from a key
to any JSON value but `null`. A key is two or more ASCII identifiers
joined by `.`, at most 128 bytes, and does not begin with `sys.` or
`lakelands.`; every integer in a value lies within ±(2^53 − 1) (§1.2).
The maps of one declaration hold at most 16 384 canonical bytes
together. An empty map is written as no member at all. A reader never
interprets an annotation: it is carried, hashed into the event like any
member, and left out of the schema hash.

A **condition** (`where`, `derived`, `constraint`, `requires`, `when`, a
measure's `expr`, an assignment's `value`, a function's `expr`) is
either a string in the text form or an object in the typed form of
Appendix B.

`migrated.migration` is `{ "to", "renames" }`: `to` is the type's new
declaration, the members of a `type` declaration without `declare`;
`renames` (default `[]`) is a list of `[from, to]` field-name pairs.
`migrated.rekeyed` (version 2, default `[]`) is a list of
`{ "type", "from", "to" }` record-key moves (§2.3).

`write_proposed.write` is the proposed write with its arguments:
`upsert` carries `type`, `key`, `patch`; `retire` carries `type`, `key`;
`call` carries `verb`, `key`, `payload`, and from version 8 on leaves
`key` out for a verb called on its type. Each also carries `opts`
(default `{}`), with `expected_version` and `idempotency_key` when given.
From version 4 on, `declare` carries `declarations`, a list of
declarations as `declared` carries one each; `migrate` carries
`migration`, as `migrated` carries it, and `report`, the preflight's
report when it was proposed (`type`, `from_schema_hash`,
`to_schema_hash`, `records_scanned`, `fields`, `dropped`, `rekeyed`,
`references`, `sample_collisions`, `lossless`, `report_id`, as the
`migrate` door answers it).

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
form exactly as here; and, from version 3 on, `date_bucket`, whose two
arguments are the value and its unit as a string literal (`day`, `week`,
`month`, `quarter` or `year`), answering the instant at which the UTC
bucket holding the value starts (the week from Monday, as ISO counts
weeks). A read may call `date_bucket` in any version; a declaration from
version 3 on (§6). From version 7 on, `retired`, with no arguments: true
when the write retires the record, in a trigger's guard only (§6). From
version 8 on, `param`, with one argument, a parameter's name as a string
literal: the value the read binds to that parameter of the view, in a
view's `where` and measures only (§6).

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
