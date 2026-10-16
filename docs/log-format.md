# The log format

**Format version 17.** Reviewed so that a verifier can be written from
this document alone (lakelands#14). A release reads and writes one
format version, this one: every event of a lake carries it, from
position 0, and a lake written in another is refused by name and never
read in part (§6).

A lake's event log is its truth (`docs/decisions/001-the-log-is-the-truth.md`).
This document specifies the log so that a lake can be read, verified and
replayed without Lakelands: the envelope every event carries, the
canonical bytes, the hash chain, every kind of event, and how the log is
cut into segments and checkpointed. The Rust form of these types is
`lakelands_api::event`; where the two disagree, this document is right.

A change to the format is a change to this document first, with the
release's one step of the version (§6), and to the code second.

`docs/log-format/vectors/<N>/` holds a worked example in format N: a
small lake's log in its stored form, with every hash, both kinds of
content-derived id, the segment digests and a view's schema hash.
`lakelands-core` recomputes them in a test, and `verify.py` beside them
recomputes them with no Lakelands code; an implementation that agrees
with both reads this document the way it is meant. The directory of
every format a release shipped is kept as that release left it, and
every later release runs it (`docs/log-format/vectors/README.md`).

## 1. Events

An event is one JSON object. Its members are the **envelope**, shared by
every kind, and `body`, which holds the kind and its payload.

### 1.1 The envelope

| Member | Type | Presence | Meaning |
|---|---|---|---|
| `v` | integer | always | The format version the event is written in: the release's one version, the same on every event of a lake from position 0 (§6). |
| `lake` | string | always | The lake the event belongs to. |
| `position` | integer | always | The event's coordinate: `0` for the lake's first event, then consecutive, with no gaps. |
| `id` | string | always | The event's identifier: 32 lower-case hex digits (§3). |
| `prev` | string | always | The `hash` of the event at `position - 1`; 64 `0` digits at position 0. |
| `known_at` | string | always | The lake's wall time when the event was appended (§1.3). |
| `writer` | string | always | The principal the event was written as, `<kind>:<id>`, where `<kind>` is `human`, `agent`, `service` or `system`: `human:alice@example.com`, `service:importer`, `system:lakelands`. The id holds no `0x00` (§3). |
| `origin` | string | always | Which authority wrote it: `source`, `authored`, `derived` or `system`. |
| `source` | string | source events | The source a `source` event came from. |
| `seen_at` | string | when known | When the source observed the values (§1.3). |
| `upstream` | object | when read from another lake | `{ "lake", "position" }`: the lake whose log the values were read from (its lake id) and the position of the event there whose state they are (an integer). Only on `record_ingested`; a reader refuses it on any other kind, naming the kind. The ingesting principal states it, as it states `seen_at`. |
| `procedure` | object | when one did | `{ "name", "version" }`, both strings, of the procedure that produced the values: for a verb, its name and its declaration's `version` in decimal digits; for a function backing, the function's name and its declaration's `version`; for an embedding, the model as the backend names it and the version it reports. |
| `context` | object | always | The host's context for the write: `{ "origin": "direct" \| "reactive" \| "scheduled", "depth": n }`, `depth` an integer ≥ 0. It never carries `cause`: a reader refuses a `context` that does, naming it. |
| `cause` | integer | consequences | The position of the event this one follows from: a verb's write names its `verb_called`, an accepted proposal's write names its `write_accepted`, a fired trigger names the write that fired it. An event of a host's write that the store sets no cause for names the position the host gave, at or below the head when it was appended (`docs/decisions/021-the-write-context-belongs-to-the-host.md`); any kind may carry it. |
| `fence` | integer | always | The fencing token of the writer lease the event was appended under (§4); `0` for the event at position 0, which is written before any lease exists in the lake. |
| `idempotency_key` | string | when given | The caller's idempotency key of an authored write. |
| `sealed` | object | from format 16, when a source's key seals a value | `{ "key", "fields" }`, or `{ "key", "source" }`: the version of the source's key, and the members of the patch it seals, or, with `source`, the recorded output it seals (§1.4). Only on `record_ingested` and `output_recorded`; a reader refuses it on any other kind, naming the kind. |
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
double, and every number written as an integer, in the envelope or in a
payload, lies within ±(2^53 − 1). That covers `position`, `version`,
`fence`, `depth`, `format`, `through`, the members of a segment, and
every `number`, `json`, `vector` or `geo` value a patch carries. Within
that range every JSON implementation reads an integer exactly and RFC
8785 writes it as its plain digits, so no two implementations disagree
on its bytes. A write carrying a larger integer is refused at the door,
never rounded; a value that needs more digits is a `decimal`.

A number is judged in the form it is written in, not the form it arrived
in. A double of 2^53 or more is integer-valued, and below 10^21 its
canonical form is its plain digits: `1e16` is written
`10000000000000000`, an integer beyond the range, and is refused like
one. From 10^21 on the canonical form has an exponent (`1e+21`), which
every implementation reads as the same double, and the number is
written. So a value a door has checked is one it checks to itself
again: one check is enough, and what is in the log passes it.

A JSON number of 10^21 or more is the double it reads as. An integer
literal that long (`1000000000000000000001`) was read as a double by
every JSON reader on its way, before the store saw it, and is written
`1e+21`: its digits are not kept. A value that needs its digits is a
`decimal`.

**The seal.** The rule holds for every event, whatever wrote it. Before
an append reaches the engine, the store judges each event it sealed: a
number anywhere in it written as an integer beyond the range, or a
`known_at` or `seen_at` out of the years of §1.3, refuses the whole
append, and nothing of it is appended. Every place a value enters
refuses it first and says where it stands:

- a field's value and a payload's member, at the field or the member;
- what a source states of a record beside its fields, at `seen_at` and
  at `upstream.position`;
- a declaration, which is written as it was declared, at the
  declaration and the member that holds the number, by its path: a
  function's `config.limits[0]`, the duration literal of an expression
  in typed form at `where.rhs.value.value`. An expression in text form
  is judged by the typed form it parses to, which is the form a schema
  hash is taken over, and said at the member that holds the text
  (`where`): a duration literal is a whole number of microseconds, so
  `104250d` is refused where `104249d` is declared. A number literal is
  kept as it is written, in both forms. The bound on a literal in text
  form is the declaring door's: a reader judges what stands in an event,
  and there text stands, so a log from another writer that holds such a
  text verifies, and its schema hash is not one every reader computes
  alike;
- a seam's output, which no caller can correct: a backing's output or
  an embedding's vector that holds such an integer is recorded as that
  call's failure (§2.5), and an executor's answer that holds one is not
  recorded, its call answered as one whose answer was lost, with the
  answer and the number.

A refusal from the seal itself names no place. It means a value passed
the place it entered at, which is a defect of that place.

**A reader judges the same.** The seal is the store's own writer's. A
log may come from elsewhere: an import, or a writer that did not seal.
A verification refuses an event that holds a number written as an
integer beyond the range as damaged at its position, naming the number
(§4.3), and an import verifies before it writes. Nobody can correct
another chain's bytes, so this is damage, not a refusal of a caller. A
`known_at` or `seen_at` out of the years is no event at all to the
parser. The fold does not judge a number that stands in an event: a
lake that holds such an event opens and answers, and its `verify` says
what is wrong. What the fold computes from events follows §2.3.

### 1.3 Time

`known_at`, `seen_at`, and every `date_time` value in a payload are UTC
instants written in exactly one form, `YYYY-MM-DDTHH:MM:SS.ffffffZ`
(microseconds, always six digits, always `Z`).

The one form is the only form in the log. The doors accept friendlier
input from callers (a date alone, fewer fractional digits, an offset such
as `+02:00`, a lower-case `t` or `z`, a space for `T`) and normalise it
to the one form before an event is written. A reader refuses an event
whose date-time is in any other form, even if it names a valid instant.

The one form has four digits for the year, so an instant lies in the
years 0000 to 9999 in UTC. An offset that would carry it out of them
(`9999-12-31T23:30:00-01:00`, half an hour into the year 10000) is
refused as no date-time, like any other text the one form cannot write.

The bound is on what is written as a date-time: a field's value, a
payload, an `initial`, `seen_at`, a cursor's value, a view's parameter.
A setting's value is JSON, and a text in it is typed as nothing: it is
written as it is and bound by nothing here. A
text that an expression only compares or computes with (a literal in a
`where`, a measure, a constraint, a function) is read as the instant it
denotes, in whatever year its offset leaves it: that literal is after
every instant a field holds, and an expression that compares with it
means what it says.

A `date` value is a calendar date written `YYYY-MM-DD`: the instant at
its UTC midnight, kept in the form a person writes it. The doors take a
date, or any instant they read above that is a UTC midnight, and write
its date; an instant at any other time of day is refused, naming the
field, since a date it was cut to would not be the value given. A date
compares and orders as its midnight, so a `date` and a `date_time` at
the same midnight are equal.

`known_at` never decreases along the log. A writer whose clock reads
earlier than the previous event's `known_at` writes the previous value
again. Only `position` orders events; `known_at` is when the lake learned
something, and it is what the fold materialises per field. A verifier
refuses a `known_at` below the one before it, naming the event's
position. The rule is what lets a reader find the position the
log had at an instant by a binary search (`LakeStore::position_at`).

`seen_at` is the source's capture time for values from a source, as the
source reported it. The lake checks that it lies in the years above and
compares it with nothing, `known_at` included.

### 1.4 Sealed values

From format 16, a source's value of a field declared `erasable` enters
the log sealed under that source's key
(`docs/decisions/058-a-source-may-be-forgotten.md`). The key is the
host's, in its key store, never in the lake; `sealed.key` names its
version, which the key store answers by.

- **The value.** The patch member holds a string: the standard base64,
  with padding, of AES-256-GCM-SIV's output over the value's canonical
  bytes (§1.2). Every other member stays plaintext, and so does the rest
  of the envelope and the body.
- **The nonce** is the first 12 bytes of SHA-256 over the source, the
  type, the record key, `version - 1` in decimal digits and the field,
  joined by `0x00`. The same value against the same state of a record
  seals to the same bytes, and two records or two versions never share a
  nonce. A reader makes the nonce again from where the value stands, so a
  sealed value moved elsewhere does not open.
- **The ingest id** is made from the patch as stored, ciphertext
  included (§3): no digest of a plaintext value enters the log.
- **A recorded output** computed from one source's erasable values is
  sealed under the same key: `sealed` names the key and the `source`,
  the `output` is a sealed value whose nonce is made from the source, the
  seam and the `input`, and the `input` is HMAC-SHA-256 under a key
  derived from the source's (HKDF-SHA-256 with an empty salt, info
  `lakelands:output:v1`), always under the version of the key the
  source first sealed with, which the fold keeps, so a rebuild finds the
  output again whatever the key store holds now. A call whose inputs hold
  erasable values of two sources is not made; the field holds no value
  and says why.
- **What equality tells.** A value's own ciphertext tells no equality
  across records or versions: its nonce is its place's. A recorded output
  is recorded once for one call (`docs/decisions/053-a-recorded-output-is-recorded-once.md`),
  so its `input` is the same for two records of one source whose call
  reads the same values: it tells that they are equal, never what they
  are, and never across two sources, whose keys differ.

The fold opens what it seals, so the projection holds plaintext. A key
the key store does not give fails the fold closed, naming the source and
the key: a lost key is never read as an erasure. A writer never asks for
a new key of a source whose sealed values the log holds when the key
store has lost the version it last sealed with: the write is refused the
same way, so a wrong key store fails at the write, not at a later
rebuild. A verifier without the
keys checks every hash and id over the bytes as stored, and the form of
each sealed member. `docs/log-format/vectors/17/` holds a sealed ingest
and a sealed output under a key its README names.

## 2. Kinds

The set of kinds is closed. A reader that meets a
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
| `lake_created` | `format` (integer) | Always at position 0, with `origin: system`, `fence: 0`, and `format` equal to the event's `v`: the format version the lake was made in (§6). |
| `lake_upgraded` | `from` (integer), `to` (integer) | A writer's open, or an import, upgraded a lake written in the older format `from`: written in `to`, which is the event's own `v`, with `origin: system`, right after the writer's `lease_taken`. The events before it stay as they were written; every event after it is in `to` (§6). |
| `lease_taken` | `fence` (integer), `holder` (string) | A writer took the lease with a fencing token greater than every earlier one. The event is the first the new writer appends: its envelope `fence` already equals its payload `fence`, and every later event carries that fence until the next `lease_taken`. `holder` is free text for the operator. |

### 2.2 Ontology

| Kind | Payload | Fold |
|---|---|---|
| `declared` | `declaration`, `version` (integer), `schema_hash` (string, types only), `with_the_set` (`true`, or absent) | A declaration enters the ontology, or replaces its earlier version, from this position on. `declaration.declare` is `type`, `value_type`, `verb`, `trigger`, `object_set` or `function`, with the declaration's members beside it. `version` is 1 for the first declaration of that `declare` and `name`, one more for each later one. A new version here never reshapes stored records. `with_the_set: true` says that another `declared` of the same set follows in this append: a set that holds a function, or a type the lake held before it, carries it on every `declared` but its last, the link types the system declares among them. The records the set's declarations compute again are computed once, at the set's last `declared`, with the ontology as the whole set leaves it; at a position inside the set they read as before it. A log never ends inside a set, where its last `declared` carries it (the memberships an object set of the set changed come after their `declared` and carry no mark), and a fold refuses one that does. |
| `value_type_migrated` | `declaration`, `version`, `renames`, `report_id`, `types` | A value type's new declaration enters the ontology at `version`, as a `declared` of it would, and every stored record of the `types` that holds an old value of `renames` (a list of `[old, new]` text pairs) in a field of the value type holds the new one, retired and restricted records included, by the fold: a field computed by a function with a backing keeps what its backing answered. A record it changes stands at this position, with its version, its flags and each field's provenance as they were; one it does not change is as it was. The pairs map each value as it stood before the event, so a chain (`a → b`, `b → c`) and a swap read the same in any order. `types` names, in name order, every type, not a view, with a field of the value type; a fold refuses an event whose `types` differs. After it, the derived fields and the fields computed by a function of those types are computed again, as of this event, as a `declared` that revises them computes them (§2.3), and their calls are owed with this event as their cause. The system's `declared` of each type whose field's `initial` named an old value follows in the same append, one set; then the set memberships the renamed records change, each caused by this event (§2.5). |
| `migrated` | `migration` (`to`, `renames`; `relation_renames` and `fill`, each left out when empty), `schema_hash`, `version`, `report_id`, `rekeyed` (`[]`) | A type's shape changes and every stored record of it is converted by the fold, by the lossless conversions the preflight report `report_id` checked. Each live record that then holds no value for a field `fill` names takes that field's `initial` from `to`, with the provenance of origin `initial` at this position (050); a retired record is left as it was. When `rekeyed` lists keys, the fold also moves those records to their new keys (§2.3). When `relation_renames` lists pairs of an old and a new name, each renamed relation keeps its edges, and the fold moves the link records of one carried by them to the link type of its new name (§2.3, "Link records"). From format 15 on, where `to` computes otherwise than the type did (its derived expressions, the functions its fields name, what its embeddings are made from, or which are computed at the read), or where `fill` is not empty and the type computes, the derived fields and the fields computed by a function of the type are computed again, as of this event, as a `declared` that revises them computes them (§2.3), and the set memberships that moves follow in the same append, each caused by this event (§2.5); a field computed by a function with a backing keeps what its backing answered until its call answers. A `migrated` of format 14 is folded as that format wrote it. |

The schema hash covers shape only (`docs/decisions/007-schema-hash-covers-shape.md`).
It is the SHA-256, as 64 lower-case hex digits, of the canonical bytes of
this object:

```
{
  "key":       [field names, in declared order; of a view, its `group_by`],
  "fields":    [{ "name", "type", "required" }, ...],
  "relations": [{ "name", "target", "cardinality", "fields": [...] }, ...],
  "view":      { "source", "where", "group_by", "measures": [{ "name", "expr" }] }
}
```

- A field is exactly `{ "name", "type", "required" }`. `type` is the
  field's whole `FieldType` object: `kind`, and `value_type`, `target`
  and `dimensions` only where the declaration sets them (absent, never
  `null`). `required` is always written, `false` when the declaration
  leaves it out. A field's `initial` is not hashed: it acts once, when a
  record is made, and changes neither how a stored record is stored nor
  how it is validated, so declaring or changing one leaves the hash as it
  was (`docs/decisions/050-a-field-declares-its-initial-value.md`).
  Nor is `erasable` (from format 16): it says how a source's values are
  kept in the log (§1.4), not what a record holds. It is refused on a key
  field, on a field that names a record, on a field only authors or the
  fold write and on a relation's field, and a field that was erasable is
  never declared without it (`docs/decisions/058-a-source-may-be-forgotten.md`).
- A relation is exactly `{ "name", "target", "cardinality", "fields" }`.
  `cardinality` is always written, `many_to_one` when the declaration
  leaves it out; `fields` is `[]` when the relation has none.
- `fields` and `relations` are sorted by name; a relation's own
  `fields` likewise. Every declared name is an ASCII identifier (a type
  name: identifiers joined by `.`), and on ASCII the order by code point
  and the order by UTF-16 code unit that §1.2 uses for members are the
  same order, so there is one sort in the whole format.
  `key` keeps its declared order, a view's that of its `group_by`,
  because the order of key fields is part of the key.
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
  `authority`, `derived`, `computed_at`, `depends_on`, `on_input_change`, a relation's
  `authority`, `via`, `inverse` and description, `valid_from` /
  `valid_to`, `language`, and `field_meta`. A relation's own `fields`
  are shape, above; so is its link type's, which has a schema hash of
  its own.

### 2.3 Records

| Kind | Payload | Fold |
|---|---|---|
| `record_upserted` | `type`, `key`, `version`, `patch`, `made` | An authored write. From format 17 its `patch` may carry a field of `authority: verb` too, written by a verb's call or by an aging rule (lakelands#1130). `patch` is sparse: each member sets a field, `null` clears it, an absent field is unchanged (`docs/decisions/012-writes-touch-only-their-fields.md`). `made` is described below the table. |
| `record_ingested` | `type`, `key`, `version`, `patch`, `made` | A write from a source; `source` is set in the envelope. Only `source` and `shared` fields may appear in the patch. `made` as on `record_upserted`. |
| `record_retired` | `type`, `key`, `version` | The record is retired: invisible to reads at or after this position, its history kept. |
| `source_forgotten` | `source`, `keys` (the versions of its key, a list) | From format 16. The host forgot the source (`docs/decisions/058-a-source-may-be-forgotten.md`): its keys are destroyed, and every value it sealed, before this position and after, folds as erased: the field holds no value, and its provenance is kept with `erased: true`; an `output_recorded` it sealed records nothing, and one that names it by `output_at` fills nothing. A sealed value whose key the key store does not give, where no `source_forgotten` of its source is in the log, is not erased: the fold fails closed. The `record_retired` of each record the source alone brought follows in the same append. A reader refuses one of a format before 16. |
| `record_restricted` | `type`, `key`, `version`, `reason` | The record is withheld from principals without the `restricted` grant (a retention hold); `reason` is the host's. Written by the host through the store facade, never by a caller's door. |
| `record_unrestricted` | `type`, `key`, `version` | The hold is lifted. |

**`made`** is `true` where the write found no live record
under the key, and is left out otherwise; `false` is never written. A
record's `version` counts through a retirement (made 1, changed 2,
retired 3, made again 4), so the two members together tell what
happened without a read:

| `made` | `version` | The write |
|---|---|---|
| `true` | 1 | made the record for the first time |
| `true` | above 1 | made it again after its retirement |
| left out | any | changed a live record |

The writer says it once, where the event is made, of what it then holds,
its own earlier events of the same append included: of two records of
one ingest that name one key, the first carries it. An accepted
proposal's write is judged when it is accepted. A key that a re-key
moved away is no record (§2.3), so a write at it carries `made` with
`version` 1. Nothing reads the member back: the fold, a rebuild, an
import and a mirror carry the event as it is.

No record event, and no `object_set_changed`, names a view, whose
records are computed, or a name that begins `sys:kept.`, under which the
store keeps a view's records
(`docs/decisions/049-a-view-the-fold-keeps.md`); no `declared` declares a
type of that form. No door writes one, and a log that holds one is not
folded past it.

`version` is the record's version after the event: 1 for the first event
that writes a key, one more for every later event that changes it. A
retired key that is written again continues its count.

**The key.** `key` is a string, the same for every event of one record
until a migration re-keys it.
For a type that declares key fields, it is made from their values in
declared order: each value in its §5 JSON form as a string (`text` and
`reference` as they are, `decimal` as its digits, `number` in the number
form of §1.2, `boolean` as `true` or `false`, `date_time` in the one
form of §1.3, `date` as `YYYY-MM-DD`), with `%` written `%25` and `:` written `%3A` inside it,
and the components joined by `:`. A key field is of one of those kinds.
The key of a `ledger_entry` keyed by `account` `DE:01` and `booked`
`2026-09-24T00:00:00.000000Z` is
`DE%3A01:2026-09-24T00%3A00%3A00.000000Z`. A writer may give the key,
the key fields, or both; when both are given they must agree, or the
write is refused. A type with no key fields takes the writer's key as it
is. No key holds `0x00` (§3).

**Re-keying.** A `migrated` event whose `rekeyed` lists
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

**Link records.** A relation declared with neither `via` nor `derived`
is carried by **link records**: ordinary records of a **link type** the store derives from
the relation, written and read by the record kinds above. No event kind
is added for links (`docs/decisions/035-a-link-is-a-record.md`).

- The link type of relation `r` on type `t` is named `sys.link.t.r`.
  It is declared by the store, as a system declaration in the same
  append as the declaration of `t` that holds the relation, and again
  whenever that declaration changes the relation.
- **A relation renamed by a migration** (`relation_renames`)
  moves its link records, with no event for a link: at the `migrated`
  event the fold takes every record of `sys.link.t.<old>`, retired and
  restricted ones too, to `sys.link.t.<new>` under the same key, with its
  version, its fields, their provenance and its flags; each key of the
  old link type ends at that position, as a re-keyed record's old key
  does, and an object set's membership moves with the record. A read as
  of an earlier position answers the links under the old name. The
  `declared` of the new link type follows the `migrated` in the same
  append, and after it the system's declaration, under the new name, of
  every trigger and verb on the old link type, every object set of it and
  every view that reads it. The old link type stays declared and holds no
  record. A migration that renames a relation carried by link records
  re-keys nothing and changes none of the relation's fields.
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
  wrong door; links are written through the relation's link doors, by
  a verb declared on the link type, which sets a link's own fields, and
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
the rules are deterministic, so a replay reproduces them. A derived
field that calls `seen_at(f)` or `known_at(f)` reads the provenance the
event leaves on `f`: the event's own where the event writes `f` or the
fold computes, clears or settles it in this event, which it does before
it computes the field that reads it; and else what `f` had. A type with
`field_meta: false` reads none. A field that
declares `computed_at: read` is not computed by the fold
and holds no value: every read computes it from the record as stored,
with its own clock (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`).

What an expression computes is judged as what is written is. A derived
field whose number would be written as an integer beyond ±(2^53 − 1)
(§1.2) holds no value, as one whose date-time would leave the years of
§1.3 does, and a verb that sets such a value is refused on the field it
sets. A migration's conversion follows: text digits beyond that range
become no number, and its preflight reports them as not convertible.
This is the rule of this log format: a derived and a converted number is
computed by it, and the fold replays a migration by it, so a reader
that folds a log computes what its writer did. Where a migration the
fold replays holds a stored value this rule does not convert, the event
is damaged at its position: the lake neither opens for writing nor
rebuilds, while `verify`, which reads the chain and not the fold, finds
the log whole.

Before it computes them, on an event that makes a record (no live record
stood under the key before it: never written, or retired), the fold
fills each field whose declaration in force declares `initial` and that
holds no value after the patch with that value. It does not count the
filled field as one the event changed: at a make every derived field is
computed anyway, and no field holds a value an input change could clear,
so nothing would read that count. The filled field's provenance is `origin: initial` with
the event's `known_at`, `position` and `writer`, and no `seen_at`,
`source`, `procedure_version` or `upstream`. A re-key does not fill. The
patch stays as written, so a reader that folds the raw feed fills the
same fields itself
(`docs/decisions/050-a-field-declares-its-initial-value.md`).

### 2.4 Verbs and proposals

| Kind | Payload | Fold |
|---|---|---|
| `verb_called` | `verb`, `key`, `payload` | A verb ran. Its writes follow as `record_upserted` events with `cause` set to this position and `procedure` naming the verb. A verb declared on its type (`scope: type`) is called with no record, and `key` is left out. |
| `write_proposed` | `proposal`, `write`, `why`, `pinned` | A write was recorded as a proposal instead of being applied (`docs/decisions/013-verbs-run-by-declared-risk.md`). `write.write` is `upsert`, `retire` or `call`, with that write's arguments; or `declare`, a set of declarations, or `migrate`, a migration with its preflight's report, or `migrate_value_type`, a value type's new declaration with its preflight's report (Appendix A) and the values it renames. `pinned: true` where the proposer's grant admits only some records of the type the write reaches: its accept is refused once the record at the key has an event after this position or the type was declared or migrated since (`docs/decisions/051-a-grant-may-admit-only-some-records.md`); absent when false. |
| `write_accepted` | `proposal` | The proposal was accepted. Its write follows in the same append, with `cause` set to this position and the proposer as `writer`: for `declare` the `declared` events, for `migrate` the `migrated` event, for `migrate_value_type` the `declared` of the value type, or, when it renames values, the `value_type_migrated` event and what follows it (the link types the store derives, and the types it declares again for their `initial`, keep `system:lakelands` as writer). |
| `write_rejected` | `proposal`, `reason` | The proposal was rejected; nothing else is written. |

### 2.5 Reactions and recorded outputs

| Kind | Payload | Fold |
|---|---|---|
| `trigger_fired` | `trigger`, `type`, `key` | A write made a trigger's condition hold. Appended in the same append as that write, with `cause` naming it. The durable fact that something is due; acting on it is the host's (`docs/decisions/004-the-change-feed-is-the-log.md`). |
| `object_set_changed` | `set`, `type`, `key`, `joined` (boolean) | A write moved a record into (`true`) or out of (`false`) an object set, in the write's append and caused by it; or a set's declaration did, in the declaration's append and caused by its `declared` (lakelands#453); or a migration did, in its append and caused by its `migrated` (lakelands#457), for a record of the migrated type, as converted and, from format 15 on, computed again (lakelands#1070), of one of its link types, or of another type whose reference to a moved key it rewrites (lakelands#465, lakelands#466); or a value type's migration did, in its append and caused by its `value_type_migrated`, for a record whose renamed values, and what is computed from them, move it (lakelands#553). |
| `output_recorded` | `seam`, `input` (digest), `output` or `output_at` (a position), `field` (optional `[type, key, field]`) | The output of a seam the store called: a function backing, an embedding backend, a verb executor. A replay reads this output instead of calling the seam (`docs/decisions/002-determinism-is-the-boundary.md`). With `output_at` it names an output the log already holds and fills `field` from it (below). |

`seam` names the seam and what was called in it: `function:<backing
kind>:<function>`, `embed:<model>` or `verb:<executor>:<verb>`. `input`
is the SHA-256 of the canonical bytes of everything the seam received
except the lake (which the event already names): for a function,
`{ "function", "inputs", "config" }`; for an embedding, `{ "texts" }`;
for a verb, `{ "verb", "record", "payload", "position" }`, `record`
null for a verb called on its type. A call that reads a source's
erasable values, directly or through the fields they are derived into,
is digested by HMAC under that source's key instead, and its output is
sealed (§1.4). Two calls with
the same `seam` and `input` are the same call, and a replay reads the
recorded output for it.

A seam that begins `sys:` is the store's own: the store keeps a kept
view's state and the lake's renames under such names in its projection,
never in the log. No `output_recorded` names one, and a log that holds
one is not folded past it.

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
several texts and failed records a failure for each of them. An output
that holds an integer beyond ±(2^53 − 1) is recorded as such a failure,
naming the number: the log writes no such integer (§1.2).

The log may hold two outputs for one `seam` + `input`: a settle forced
by a declaration calls again for an input whose recorded output is a
failure (`docs/decisions/022-functions-are-declarations.md`, extended).
The later output holds. The fold, and a projection's recorded outputs,
answer the latest output recorded for a call from its position on, and
the earlier one before it.

A call's output is in the log once per answer. Where an output the lake
holds fills a field again (two records whose texts are the same, a
record whose fields return to a text embedded before), the event names
it by `output_at`, the position of the `output_recorded` that carries
it, and carries no `output`:

```json
{"kind":"output_recorded","seam":"embed:m","input":"6ff3…","output_at":1234,"field":["doc","d000042","near"]}
```

`output_at` names the call's latest output at this event's position:
the same `seam` and `input`, an event that carries `output`. The fold
fills `field` from that output and records no output of its own, so the
projection's recorded outputs are those of the events that carry one.
An event that carries both `output` and `output_at`, or neither, is
refused; so is an `output_at` that names any other position, as a
damaged log, never folded past. A failure, a new answer and a settle's
second output for a call carry `output`. A verb's output fills no
field, and is carried.

The event at `output_at` is the fill of another record's field, or of
the same record before. A reader that resolves it reads the log, and
every reader of the log reads it whole: the feed is the log as it is,
and opens over a network door only to a principal that reads every
record (`docs/decisions/037-the-feed-over-a-network-door.md`), and an
export holds every event (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
So a consumer that may read the fill may read the event it names. The
log is append-only, purge removes no event and a sealed segment never
changes, so the event at `output_at` is there as long as the reference
is (`docs/decisions/053-a-recorded-output-is-recorded-once.md`).

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
- **A verb's recorded answer**: `SHA-256("lakelands:output:v1" ‖ 0x00 ‖
  lake ‖ 0x00 ‖ position)`, where `position` is that of the event that
  made the verb due (its `verb_called`, or the `write_accepted` of its
  acceptance), in decimal digits. Such an event is the `output_recorded`
  of a verb's executor; a position has one. A retry under the call's key
  finds the answer by this id, however far the log has grown.
- **An acceptance**: `SHA-256("lakelands:accepted:v1" ‖ 0x00 ‖ lake ‖
  0x00 ‖ proposal)`, the `write_accepted` of that proposal; a proposal is
  accepted once, and an accept sent again finds it by this id.
- **Every other event**: any id not used before in the lake. Lakelands'
  store derives it as `SHA-256("lakelands:event:v1" ‖ 0x00 ‖ lake ‖ 0x00
  ‖ position)`, the position in decimal digits: a position is used once
  in a lake, so the id is unique without a source of randomness. A
  reader never recomputes these ids; it only requires them unique.

The aging, answer and acceptance ids add no kind, member or encoding. A
reader that does not recompute them still reads the event. In a lake written before them, a verb's answers and the
acceptances stand under positional ids: a retry there finds no answer and
is refused as `unanswered`, and an accept sent again is `decided`;
nothing runs twice.

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

A reader also refuses an event that holds a number written as an integer
beyond ±(2^53 − 1), at that event's position (§1.2): its bytes may be
canonical and its hash right for one implementation, and another, which
reads the number as a double, would write other bytes and compute
another hash. `LakeStore::verify`, the verification of an export and an
import refuse it alike.

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
| `date` | string, `YYYY-MM-DD` (§1.3) |
| `bytes` | string, standard base64 (RFC 4648 §4) with padding |
| `json` | any JSON value, under the rule of §1.2 |
| `reference` | string, the target record's key |
| `vector` | array of numbers, exactly as many as the field's `dimensions`, each finite as a 32-bit float |
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

Every `vector` element is finite as a 32-bit float, the form a search
ranks by; otherwise every search of the field would score the record
`NaN`. A write holding `1e300` is refused, naming the field; a
migration counts a stored value holding it as one that does not
convert; a function's output holding it fills nothing and names the
field in its `error` (§2.5); and a derived value holding it, or any
value its field's type would not take, lands no value. `1e-200` is a value, 0 as a 32-bit
float, and so is a vector of zeros.

## 6. Versioning

**A release writes one format version, and reads every one from 14 to
it.** Every event carries the version it was written in as `v`, and
`lake_created.format` says the version the lake was made in, the same
as its own line's. Within a version the format is fixed and described
whole: this document describes the version the release writes, and
each earlier one is read through its converter (below).

**The version rises by one in a release that changes the format, and
only there.** A new kind, a new member or a changed encoding is a change
of the format, specified here first; a release that makes several takes
one step for all of them, and a release that makes none keeps the number.
Between two releases the format is open: the first change raises the
number, and later ones change the format under it, so a lake written by
a build between two releases may not open in the next.

**Every step from 14 on carries its upgrade path.** 14 is the format of
the first release with real installations
(`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).

- **A converter a step.** Each step ships a function that takes an
  event's members as the earlier format wrote them and gives them as the
  later one says them. A reader takes an event's `v` first and runs the
  converters from it up to the release's format, then reads the event.
  The event keeps its `v`, its bytes and its hash: the log is never
  rewritten, and a hash is taken over the bytes as they were written
  (§1.2), which no step changes.
- **The steps so far.** 14 → 15: none of an event's members change.
  Format 15 lets a verb's path read across a reference or a relation's
  link (`docs/decisions/055-a-path-steps-through-a-reference.md`), and a
  log of format 14 holds no declaration that does; a value type may
  carry `annotations`, which no value type of format 14 holds. 15 → 16:
  none either. Format 16 is 0.15.0's one step; each form it adds is
  said here by the change that adds it, and a log of format 15 holds
  none of them: an event's `sealed` and a field's `erasable` (§1.4,
  `docs/decisions/058-a-source-may-be-forgotten.md`), so each value of
  format 15 is plaintext; and a set's `where` and a trigger's `when`
  that read across a reference, a relation's link or a derived relation,
  and ask `exists` (055). 16 → 17: none. Format 17 is 0.16.0's one
  step, taken before any form it adds (lakelands#1164); each is said
  here by the change that adds it, and a log of format 16 holds none of
  them. No step changes a fold, so the projection stays. A step that changes how an event folds
  says so here, and a writer's open then rebuilds the projection.
- **A writer's open upgrades.** Where the newest event, read under the
  writer's lease, is in an older format the release reads, the open
  appends, right after its `lease_taken`, one `lake_upgraded` from that
  format to the release's, in the release's; from there the lake is
  written in the new format. Another writer that upgraded the lake
  between this open's look at the log's ends and its lease leaves it
  nothing to mark. `lakelands upgrade` is that open as a command, and
  `--check` says what it would do and opens nothing. An import of an
  export of an older format imports its events as they are, and a
  writer's open then upgrades the new lake, under a lease above every
  fence the export holds.
- **A reader is refused.** A read-only open of a lake whose newest event
  is in an older format the release reads is refused before anything is
  read, naming the lake, both numbers and the way: a writer's open, or
  `lakelands upgrade` (`log_upgrade_owed` in `docs/refusals.md`). A
  reader appends nothing, and an older projection is not read with newer
  code.
- **A format changes only where a new writer upgrades it.** Along a log
  `v` never falls. It rises only at a new writer's `lease_taken`, the
  first event that writer appends, and that writer's next event is the
  `lake_upgraded` from the format before to the new one; `verify` and
  the verification of an export refuse any other change at its position,
  and a rise its marker does not follow where the log ends
  (`format_step`); a read refuses an event in a format below the one
  before it where it reads them in order. A `verify` of a range judges
  the step only from where it begins: a `verify` of the whole log judges
  every one. A fence below the one before it is refused by both engines
  at append and at import, and found by `verify` and by the verification
  of an export where it was written below them (`fence_below`, §4).

**A lake in a format the release does not read is refused, older or
newer, and never read in part.** A reader reads an event's `v` before
any other member of it; a line with no `v` is in format 0. (A
verification of an export checks first that the line is its canonical
bytes, and `v` next.)

- An open of a lake whose first event, its `lake_created` or its newest
  event is in a format the release does not read is refused before the
  lake is opened, naming the lake, that format, the formats read and the
  one written (`log_format`). So is a lake whose newest event is in a
  format below its first's. Nothing of the lake is judged, made, folded
  or appended to. The lake is not damaged, and restoring it would not
  help: it is another release's.
- Wherever else an event is read (a verification of the lake or of an
  export, the feed, a rebuild, an import), one in a format the release
  does not read is refused at its position, naming both numbers
  (`format_other`).
- A `lake_created` that says another format than its own line, or one
  the release does not read, is refused where it is read by a flaw of
  its own (`created_in_another`), at whatever position it stands; a
  store writes it at position 0 alone.

**There is no downgrade.** A lake a release upgraded is not opened by
the release before; a backup taken before the upgrade is the way back.

**A lake the release before wrote is kept and read.**
`docs/log-format/lakes/<N>/` holds a small lake a release of format N
made through its doors, exported, and what that release answered of it;
every later release imports it on every engine, upgrades it and answers
the same. The release that makes one keeps it too, read in its own format
with no upgrade, so its answers are held from the day it ships.

A lake written before format 14, when one log held versions 1 to 14 in
rising order, is not read.

## Appendix A. Encodings

Every enumerated value in the log is one of the lower-case strings
below; no other spelling is read.

| Where | Values |
|---|---|
| a field type's `kind` (§5) | `text`, `number`, `decimal`, `boolean`, `date_time`, `bytes`, `json`, `reference`, `vector`, `geo`, `date` |
| an event's `origin` | `source`, `authored`, `derived`, `system` |
| a field's provenance `origin` (§2.3) | an event's, and `initial`, which an event's own `origin` never is: a log whose event carries it is refused at that event |
| `context.origin` | `direct`, `reactive`, `scheduled` |
| the kind in `writer` | `human`, `agent`, `service`, `system` |
| a field's `authority` | `authored`, `source`, `shared`, `derived`, `verb` (from format 17) |
| a field's `on_input_change` | `keep`, `clear` |
| a field's `computed_at` | `write`, `read` |
| a relation's `cardinality` | `one_to_one`, `one_to_many`, `many_to_one`, `many_to_many` |
| a verb's `risk` | `low`, `medium`, `high` |
| a constraint's `at_ingest` (default `refuse`, which is never written) | `refuse`, `admit` |
| a verb's `effect` | `write` (with `set`), `external` (with `executor`) |
| a verb's `scope` (default `record`, which is never written) | `record`, `type` |
| `declaration.declare` | `type`, `value_type`, `verb`, `trigger`, `object_set`, `function` |
| a proposal's `write.write` | `upsert`, `retire`, `call` |

A **declaration** is an object whose `declare` names what it declares,
with that declaration's members beside it. Every member not marked
"required" may be absent, and an absent member means its default:

| Declaration | Members (default when absent) |
|---|---|
| `type` | `name` (required), `key` (`[]`), `fields` (`[]`), `relations` (`[]`), `view` (none), `valid_from`, `valid_to`, `language` (none), `field_meta` (`true`), `insert_only` (`false`), `description`, `label` (none), `annotations` (`{}`), `constraints` (`[]`: each `{ "name", "holds" }` required, `description` (none), `at_ingest` (`refuse`)) |
| a field (in `fields`, a relation's `fields`, a verb's `payload`, a function's `inputs`) | `name` and `type` (required), `required` (`false`), `authority` (`authored`), `derived` (none), `computed_at` (none), `depends_on` (`[]`), `on_input_change` (`keep`), `initial` (none; on a type's own field alone), `sensitive` (`false`), `searchable` (`false`), `description`, `label` (none), `annotations` (`{}`) |
| a field's `type` | `kind` (required), `value_type`, `target`, `dimensions` (none) |
| a relation | `name` and `target` (required), `cardinality` (`many_to_one`), `authority` (`authored`; who writes its links, when link records carry it), `inverse`, `via` (none), `fields` (`[]`), `description` (none), `annotations` (`{}`), `derived` (none: `{ "match": [ { "from", "to" } ], "where" }`, a field of the owner and one of the target per pair, and `where` (none) a condition over `from.<field>` and `to.<field>`) |
| a `view` | `source` and `measures` (required; each measure `{ "name", "expr" }`), `where` (none), `group_by` (`[]`), `params` (`[]`: each `{ "name", "kind" }`, a kind of `text`, `number`, `decimal`, `boolean`, `date_time` or `date`), `merge` (`[]`: each `{ "source", "where", "group_by", "measures" }`; `source` and `measures` required, `where` none, `group_by` `[]`) |
| `value_type` | `name` and `kind` (required), `constraint` (none), `one_of` (`[]`), `format`, `scale`, `description` (none), `annotations` (`{}`) |
| `verb` | `name`, `on`, `risk` and `effect` (required), `payload` (`[]`), `requires` (none), `description` (none); `effect: write` carries `set`, a list of `{ "field", "value" }`; `effect: external` carries `executor` |
| `trigger` | `name` and `on` (required), `when`, `description` (none), `max_depth` (none: an integer at most the lake's `max_reactive_depth`) |
| `object_set` | `name`, `of` and `where` (required), `description` (none) |
| `function` | `name` and `backing` (required), `inputs` (`[]`), `expr` (none), `config` (`null`), `description` (none) |

An **annotation map** (`annotations`) is an object from a key
to any JSON value but `null`. A key is two or more ASCII identifiers
joined by `.`, at most 128 bytes, and does not begin with `sys.` or
`lakelands.`; every integer in a value lies within ±(2^53 − 1) (§1.2).
The maps of one declaration hold at most 16 384 canonical bytes
together. An empty map is written as no member at all. A reader never
interprets an annotation: it is carried, hashed into the event like any
member, and left out of the schema hash.

A **condition** (`where`, `derived`, `constraint`, `holds`, `requires`, `when`, a
measure's `expr`, an assignment's `value`, a function's `expr`) is
either a string in the text form or an object in the typed form of
Appendix B.

`migrated.migration` is `{ "to", "renames" }`: `to` is the type's new
declaration, the members of a `type` declaration without `declare`;
`renames` (default `[]`) is a list of `[from, to]` field-name pairs.
`migrated.rekeyed` (default `[]`) is a list of
`{ "type", "from", "to" }` record-key moves (§2.3).

`write_proposed.write` is the proposed write with its arguments:
`upsert` carries `type`, `key`, `patch`; `retire` carries `type`, `key`;
`call` carries `verb`, `key`, `payload`, and leaves
`key` out for a verb called on its type. Each also carries `opts`
(default `{}`), with `expected_version` and `idempotency_key` when given.
`declare` carries `declarations`, a list of
declarations as `declared` carries one each; `migrate` carries
`migration`, as `migrated` carries it, and `report`, the preflight's
report when it was proposed (`type`, `from_schema_hash`,
`to_schema_hash`, `records_scanned`, `fields`, `dropped`, `rekeyed`,
`references`, `sample_collisions`, `lossless`, `report_id`,
`restated`, left out when empty: a list of
`{ "field", "member", "was", "to" }`, `field` left out for a member of
the type itself, and `breaches`, left out when empty: a list of
`{ "type", "constraint", "records", "sample_keys" }`, as the
`migrate` door answers it). `migrate_value_type`
carries `to`, the value type's new declaration as `declared` carries
one, and `report`, its preflight's report when it was proposed
(`value_type`, `from_version`, `records_scanned`, `fields`, `misfits`,
`first`, left out when empty, `fits`, `report_id`, `breaches`, left
out when empty, as a migration's report holds it, as the
`migrate_value_type` door answers it). A logged report fits, so its
`first` is empty. `migrate_value_type` also carries
`renames`, left out when empty, a list of `[old, new]` text pairs, and
its report the members `conditions` (a list of strings) and
`conditions_unshown` (`true`, or absent), and, in each of `fields`,
`renamed`, a list of `{ "from", "to", "records" }`; each is left out
when empty, so a migration that renames nothing reads as before.

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

A path's first identifier names a field of the record the expression is
evaluated on, or `payload` in a verb's values; in a derived relation's
`where` it is `from` or `to`, the owner's record or the target's, and
the second a field there. Each later one reads the
member of that name of the value the path has come to; in a verb's
`requires` and set values, a set's `where` and a trigger's `when`, where
that value is a reference, it reads the field of that name on the
record the reference names; and there a
first identifier that names no field but a relation carried by link
records reads the one link the record holds under it, the next
identifier a field of that link, and a first identifier that names a
derived relation onto its target's key reads the one record it matches
(`docs/decisions/055-a-path-steps-through-a-reference.md`). `exists`,
with one argument, a relation's name: whether the path that begins
there reaches a record, in a verb's `requires` and set values, a set's
`where` and a trigger's `when` only. These readings came with format 15
for a verb, and with format 16 for a set and a trigger: a log of an
earlier format holds no such declaration.

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
`setting`, `changed`, `exists`, `coalesce`, `if`, `lower`, `upper`, `trim`, `len`,
`concat`, `substr`, `contains`, `replace`, `starts_with`, `matches`,
`abs`, `round`, `count`, `sum`, `min`, `max`, `avg`, spelled in the text
form exactly as here; and `date_bucket`, whose two
arguments are the value and its unit as a string literal (`day`, `week`,
`month`, `quarter` or `year`), answering the instant at which the UTC
bucket holding the value starts (the week from Monday, as ISO counts
weeks). `retired`, with no arguments: true
when the write retires the record, in a trigger's guard only. `param`, with one argument, a parameter's name as a string
literal: the value the read binds to that parameter of the view, in a
view's `where` and measures only. `floor`, with
one argument: the greatest whole number not above it; and `date_bucket`
with three arguments, the value, a width as a duration literal above
zero, and the origin: the start of the window of that width holding the
value, the windows counted from the origin in both directions,
`origin + floor((value - origin) / width) * width`. A calendar unit
takes no third argument and a width takes one. And `in_set`, with one
argument, an object set's name as a string literal: true for a record
where that set's filter holds for it, the filter read in the call's
place when the condition is read, never stored membership, but in a
read's `where` a set whose own reads across an edge, which is answered
by its kept membership
(`docs/decisions/055-a-path-steps-through-a-reference.md`); in an object
set's `where`, a trigger's `when` and a read's `where` only, a set of
the record's type, and no set through itself.

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

`docs/log-format/vectors/17/schema-hash.json` shows a view whose conditions
are written as text, its shape object with those conditions in typed
form, and the resulting hash. A shape's `key` is the key the type's
records carry, so a grouped view hashes alike whether its declaration
writes its `key` or leaves it out; the vector holds that hash too.
