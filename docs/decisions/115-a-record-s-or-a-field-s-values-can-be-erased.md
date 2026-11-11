# 115 — A record's or a field's values can be erased from its history

## What is decided

### One act erases a record, a field of it, or a quarantined row

A host may erase the values of one record, of one field of one record, or
of one quarantined row, without erasing the rest of the source, the type
or the lake. The act is a method of the store facade, beside
`remove_values` and `thin_values` (`crates/lakelands-api/src/catalog.rs:705`,
`:716`, `:730`):

```rust
async fn erase_values(
    &self,
    selection: &Selection,
    require_shredded: bool,
    report_id: Option<&str>,
    preflight: bool,
) -> Result<crate::write::EraseReport>
```

It is one method with a `preflight` flag, as `thin_values` is, and it is
provided with a default that answers `Error::unbound`, as `thin_values`'
default does (`catalog.rs:736`). An engine that does not port it keeps
building, and a handle that is no host's answers it unbound.

It is the host's act, as taking part of a source back is: there is no
door and no grant, and a caller cannot reach it. It runs under the
writer's lock, appends once, and its report names the position.

### Two selections name what it takes

`Selection` (`crates/lakelands-api/src/write.rs:2145`) gains two members,
both additive. Its doc becomes *which values an act takes back*, since
`remove_values` and `erase_values` now read the same enum.

- `Record { ty: TypeName, key: RecordKey, field: Option<FieldName> }`,
  written `{"record": {"type": …, "key": …, "field": …}}`; `ty` carries
  `#[serde(rename = "type")]`, so the member is written `"type"` as a
  record's is, not `"ty"`; `field` absent says every field of the
  record. The key is the record's key as the head reads it. The act
  follows the record's identity backwards through every
  re-keying the log records for it (`crates/lakelands-api/src/event.rs:209`)
  and selects every `record_upserted` and `record_ingested` of its type at
  a key it held, before this position, that wrote the record. A key that
  another record has taken ends the chain: the writes after that are not
  this record's.
- `Quarantined(crate::quarantine::QuarantineId)`, written
  `{"quarantined": {"source": …, "delivery": …, "index": …}}`. It selects
  every version of the id.

A record selection takes the values of every hand that wrote the record,
authored and sourced alike, not one source's. `remove_values` keeps its
scope: it takes only the values of the source it names, and it refuses
either new member, naming `erase_values`. Its contract is unchanged.

### `values_removed` does not carry the two new members

`Selection` is also the `selection` member of `values_removed`
(`crates/lakelands-api/src/event.rs:282`), where `Removal::of` reads it
(`crates/lakelands-store/src/remove.rs:117`) and `Removal::selects`
matches its four members (`remove.rs:153`). Only those four belong
there: a removal selects by its source, and a record or quarantined
selection names no source and could not be folded by it.

The reader refuses a `values_removed` whose selection is a `record` or a
`quarantined` selection, rather than letting the fold meet it and
quietly select nothing. The build adds the check to `misplaced`
(`crates/lakelands-core/src/log.rs:281`), beside the other
member-where-it-never-sits flaws and under a `Flaw` that names the
selection, so the log's reader and a rebuild both refuse the event as
damage, never skip it. §2's closed set and §2.3's `values_removed` row
come to say the same: its `selection` is one of the four. The arm the
fold's match gains for the two members is unreachable, and answers the
same refusal where it is ever reached.

### What the act never takes

- A key field and a reference field, as for a removal
  (`crates/lakelands-store/src/remove.rs:183`).
- A field's `initial` value: that is the declaration's, not a value of
  the record (`docs/decisions/050-a-field-declares-its-initial-value.md`).
- A `sys.` type: the lake's own bookkeeping is no host's to erase, and
  the act refuses such a selection as it refuses an unreadable one.

What it leaves is counted in the report's `kept`, and it keeps the
record alive.

### The act does not retire the record

A record left holding no value stays live and reads with every value
erased, its provenance kept with `erased: true`. Retirement stays what
065 made it: a consequence for a record *left* holding nothing by a
selection that did not name it. This act names the record, so its
existence is the host's to keep. 065 and 069 are unchanged here.

A field selection never retires, and an erase that would take nothing —
every selected field already erased, or only key, reference or `initial`
values left — appends nothing and answers a report that says so.

### One event, and the format's next step

The act appends one event of a new kind, `values_erased`, in §2 of
`docs/log-format.md`:

| Kind | Payload | Fold |
|---|---|---|
| `values_erased` | `selection` (a `record` or a `quarantined` selection, as above), `keys` (the versions of the record's key it destroys, oldest first; absent where none), `apart` (the apart keys it destroys, in id order; absent where none) | The host erased values of one record, one field or one quarantined row (`docs/decisions/115-a-record-s-or-a-field-s-values-can-be-erased.md`). Each non-null member of the patch of every selected write before this position, and every cell of every version of a quarantined id, folds as erased at its own position, `as_of` included; a key field, a reference field and an `initial` excepted. A value sealed under a version `keys` names, or under a key `apart` names, which the key store no longer gives, folds as erased, not as a lost key. Derived values, sets and triggers over an erased value are recomputed or waited as for `values_removed`. |

A new kind is a change of the format (§6): the build raises
`LOG_FORMAT_VERSION` and `OLDEST_LOG_FORMAT`
(`crates/lakelands-api/src/event.rs:26`, `:31`) from 23 to 24, one step
for the release, as 0.24.0 makes it. It adds the kind to the closed set
of §2, a vector set under `docs/log-format/vectors/24/` — an erase by
record, by field, by quarantine id, and one that destroys a key and an
apart key — and, at the 0.24.0 release, the lake that release makes
through its doors under `docs/log-format/lakes/24/`, as each release
keeps the lake of its format; the build itself adds none. No format before
24 is read (`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`).

### A close may erase in the same append

A lake setting, `erase_on_close`, default off, is the host's
configuration, held as the lake's own `sys.setting` record as every lake
setting is (`docs/decisions/063-every-setting-is-in-one-list.md`), never
read from the environment. When it is on, every `quarantine_closed` an
ordinary close appends is followed in the same append by one
`values_erased` for its id. The erase is `system:lakelands`'s in every
case, a `Draft::system` beside the close
(`crates/lakelands-store/src/log.rs:68`), whatever writer wrote the
close, and the append carries both or neither:

- A close by landing is `system:lakelands`'s
  (`crates/lakelands-store/src/quarantine.rs:330`).
- A close by resubmission is written in the resubmission's own append,
  under the resubmitting writer and not `system:lakelands`'s
  (`crates/lakelands-store/src/ingest.rs:553`, `docs/log-format.md:563`);
  the erase is `system:lakelands`'s beside it in that append.
- A close by dismiss is under the dismisser as writer, origin `authored`
  (`crates/lakelands-store/src/quarantine.rs:394`); the erase is
  `system:lakelands`'s beside it, not the dismisser's.

No caller's grant selects an erasure, no caller supplies its selection,
and no new verb is asked of the host: dismissing, landing or resubmitting
closes, and the configured close erases.

- A close by `source_forgotten`, by a `values_removed` that selects the
  entry, by a purge, or by this act's own erase of an open entry already
  erases through its own event; no second erase is appended.
- Erasing an open entry through the act appends the `values_erased` and a
  `quarantine_closed` with `why: removed` in the same append
  (`ClosedWhy::Removed`, `crates/lakelands-api/src/quarantine.rs:59`).
  Erasing a closed one appends no close and leaves its first close.
- 085's wording is amended: `removed` says the entry's values were taken
  back, and the erase reaches every version of the id, not only its last,
  because a read at an earlier position must not answer them. 085's "not
  an erasure" line comes to point here: the close erases nothing by
  itself, and this act's `values_erased` erases.

### What "erased" means, and what it does not

Redaction is the floor. Shredding is the ceiling, and only the key
decides it.

- **Erased** means no read returns the value: the head, every `as_of`,
  `get`, `find`, a set, a derived value, and the projection holds none.
  It does not mean the bytes are gone.
- **Shredded** means the sealed key that held the value is destroyed, so
  the canonical bytes no longer open it. A key is destroyed only where it
  seals no value outside the selection: for a quarantined entry, its
  delivery's own apart key where it has one
  (`docs/decisions/087-an-ingest-may-carry-a-delivery-per-record-each-sealed-apart-under-a-key-of-its-own.md`); a key a
  delivery sealed apart is in no chain, and a shared file key that also
  seals a neighbour's values is not destroyed.
- **Plaintext at rest is not erased.** A value never sealed, or sealed
  under a key that also seals a value outside the selection, is
  `unshredded`; `require_shredded` refuses the act while any is. This is
  the answer to the question the issue leaves this record: a quarantined
  cell is plaintext in the log (085), so a close erases it from every
  read and from the projection, not from the log's bytes. A host that
  needs the bytes gone under a key it can name asks `require_shredded`
  and is answered "no" unless the delivery sealed the row apart.
- A copy outside the lake — an export, a backup taken through a position
  before the act, the host's own copy — still carries the bytes, and
  this act does not reach it.

### Verify, export, import, rebuild, mirrors

- **Verify** is unchanged: it recomputes every hash and canonical form
  over every event, the new kind included, and judges no erasure
  (`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`).
- **Export** carries the event. Keys are exported apart
  (`docs/decisions/077-a-backup-is-an-export-that-grows.md`), so a
  restore folds the event, erases as the lake did, and destroys the
  restored key at its finish. A restore `--through` a position before
  the act reads the values, as a read at that position does.
- **Rebuild** folds the event as any other. The keys it destroys are
  written into the projection's removed-key list (`sys:removed`,
  `crates/lakelands-store/src/remove.rs:64`), which `values_removed`
  already keeps, so a value whose key is gone reads erased and not lost.
  No projection table is added.
- **Mirrors**: a mirror of a source that wrote a value in the erased
  record stops, as a removal's mirror does. The report names the sources.

### The report

`EraseReport` is a new type, additive to the contract because nothing
existing changes: `selection`, `types` and `fields` (each with
`redacted` and `shredded`, as `RemovedField` has), `versions` (the writes
it selected), `keys`, `apart`, `kept`, `references`, `unshredded`,
`recomputed`, `views`, `breaches`, `log_events`, `position`, `report_id`,
`quarantined` (the close it appended, if any), `sources` and `said()`. A
`preflight` answers the same counts and appends nothing, under the
report's id where one is given.

### The contract it changes

- **Additive under 056.** A new enum member read by a new act, a new
  event kind, a new report, a provided method. No door, no grant, and no
  existing member changes meaning. The only edit to an existing contract
  is `Selection`'s doc, which no host reads as behaviour.
- **Not breaking for a host or an engine.** The selections are new, the
  act is provided and answers `unbound` where unported, and the format
  step is the release's own.
- **Breaking for an exhaustive match on `Selection`.** A tool in the tree
  that matches it must gain two arms; 056 counts that addition as free
  and no deprecation precedes it. `remove_values` refuses the two new
  members in words that name `erase_values`, so a caller that passes one
  by mistake is told what to call.

### What it takes to build

Measured at develop `84b642a3`: product 114 998 of base 115 000, so 2
lines are free; specification 202 995 of base 203 000, so 5 are free;
seam width 111 of 111, so none is. The build therefore needs three rows
in `docs/budgets.md` under 047 — product 115 000 → 116 000,
specification 203 000 → 204 000, seam width 111 → 112 — each with its
measured count rounded up to the next 500 and with the cutting that came
first.

- **Product, about 600–850 lines**: the act and its walk, reusing
  `Removal`, `Fixed` and the removal's resolution
  (`crates/lakelands-store/src/remove.rs:71`, `:98`, `:117`, `:339`) for
  the key chain instead of a second walk; the event and its fold; the
  report; the close wiring in `crates/lakelands-store/src/quarantine.rs`
  and `crates/lakelands-store/src/ingest.rs`; the `values_removed`
  refusal in `misplaced` (`crates/lakelands-core/src/log.rs:281`); the
  `.unshredded` and `kept` accounting; and the setting's three pieces
  the `settings` gate reads (063, Gate 16): its `SETTINGS` entry in
  `crates/lakelands-api/src/settings.rs:102` (`..lake("erase_on_close",
  PerType::No, "false")` with its `range` and `says`), the
  `ERASE_ON_CLOSE_SETTING` constant in `crates/lakelands-api/src/seam.rs`
  beside `ZONE_SETTING` (`seam.rs:748`), and the one product read of the
  setting by that constant, since a listed lake setting no product file
  reads is red.
- **Specification, about 450–650 lines**: the rows below, in
  `crates/lakelands-core/src/conformance/checks/`, one file beside the
  removal's and the quarantine's.
- **Docs, about 250 lines**: the §2.3 rows and the §6 step in
  `docs/log-format.md` (the new kind, and `values_removed`'s selection
  set closed to its four), the vectors and the lake, the regenerated
  `docs/settings.md` (`scripts/settings.py`), and the amendments to 065
  and 085 named above.

The cut comes first: the act reuses the removal's resolution, its
`Fixed`/`Removal` types and the `sys:removed` list rather than growing a
second walk, a second retirement path and a second projection table; the
report reuses `RemovedField`'s shape; and the close appends the event
rather than gaining a verb.

### The rows the suite gains

1. An erase by record erases every value of the record, and no read
   returns one at the head or at any earlier position.
2. An erase by field erases that field's values and leaves the record's
   other values standing.
3. Key fields, reference fields and `initial` values are never taken,
   and are counted in `kept`.
4. The act takes the values of every hand: an authored value and a
   sourced value of the same record go; `remove_values` still takes only
   its source's.
5. A record left holding no value stays live and reads with its values
   erased and its provenance kept.
6. An erase that would take nothing appends nothing and says so.
7. An erase by quarantine id erases every version's cells at its own
   position, and closes an open entry `removed` in the same append.
8. Erasing a closed entry leaves its first close and appends no close.
9. `require_shredded` refuses while a value is plaintext or is sealed
   under a key that seals a value outside the selection; the report lists
   it in `unshredded`.
10. Where the delivery sealed the row apart, the key is destroyed, and a
    restored log reads the cells erased and not as lost keys.
11. `remove_values` refuses a record or quarantined selection, naming
    `erase_values`.
12. With `erase_on_close` set, a dismiss appends `values_erased` beside
    `quarantine_closed` in one append, and the host hears afterwards.
13. With it set, a close by landing erases the quarantined copy in the
    same append; with it unset, the close erases nothing.
14. Export, import, restore and rebuild answer the same erased values,
    and the restored key is destroyed at the restore's finish.
15. Verify over a log with the new kind answers unchanged, and a reader
    that does not know the kind refuses the event rather than skipping it.
16. A mirror of a source that wrote a value in the erased record stops,
    and the report names the source.
17. A `values_removed` whose selection is a record or a quarantined
    selection is refused as damage by the reader and by a rebuild, and
    no fold treats it as selecting nothing.
18. An erase by record follows the record's identity backwards through
    every re-keying the log records, and selects the writes at each key
    it held before the act.
19. A key another record has taken ends the chain: the writes at that
    key after the takeover are not selected and still read.
20. A selection of a `sys.` type is refused, as an unreadable selection
    is, and the log's head does not move.
21. A preflight appends nothing, answers the same counts the act would,
    and leaves the head where it was.
22. With `erase_on_close` set, a resubmitted close erases in the
    resubmission's own append, the erase written `system:lakelands`, and
    the resubmitter's grant is not asked for it; with it unset, the
    close erases nothing.

## Why

A close today hides a row and erases nothing. Every read already omits a
closed row's cells, and 085 is exact that a dismiss "closes and erases
nothing"; the values stay in the log under the delivery's seal, in the
projection and in a host's copy. A host that refuses a file and keeps a
bad row needs a way to take that row's values back without taking the
file back as one: taking the file back would erase rows the host wanted,
and a per-row apart delivery separates the row only by breaking the file
back into parts (087). The act gives the host the unit it already
reasons about — a record, a field of it, a row — and keeps the source's
own selection as it was.

The close that erases is what makes the act usable in the ordinary case.
A host hears of a close after the lake made it, so an erase the host had
to ask for would lag the close and need a new data verb; an erase the
lake appends with the close is one act, under one lock, and a crash
leaves both or neither.

The record does not pretend to remove bytes. The log is append-only and
its chain is what a verify recomputes; 065 already says an act does not
remove bytes from the log, and this one says what it does remove: every
read's answer, the projection, and, where the key allows, the seal. The
host that needs the bytes gone is told the truth — `require_shredded`
refuses rather than pretending.

## What it is not

- It is not an erasure of bytes from the log, an export or a backup
  (065).
- It does not reach a copy outside the lake (065).
- It does not retire the record it names, and it closes no entry it did
  not name.
- It is not a hiding or a restriction: a hidden value is there to one
  with the grant (`docs/decisions/109-what-is-hidden-answers-as-what-is-not-declared.md`);
  an erased one is nowhere.
- It does not select by type, by value or by condition: only a record, a
  field of it, or a quarantined id.
- It is not a door: no grant selects it, and a caller cannot erase what a
  host did not ask.
- It does not change `remove_values`: that act still selects by its
  source alone, and still retires what it leaves empty.

## How it came to be

lakelands#1736. 085 kept a refused row whole and taught every read to
omit its cells, which is a hiding and not an erasure; the host asked for
the values themselves to be erasable, and for the close to be able to
erase them. The two additions to `Selection`, the refusal in
`remove_values` and the `removed` close come with it. The two new
selections are refused in `values_removed` and not folded there: a
removal selects by its source, so the format's closed set keeps its four
members. 065's and 085's rules are amended in the same build where this
act lands: 065's doc for `Selection`, and 085's line on the close and
its "not an erasure".
