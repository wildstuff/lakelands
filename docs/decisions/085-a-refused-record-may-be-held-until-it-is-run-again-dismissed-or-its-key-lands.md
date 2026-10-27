# 085 — A refused record may be held until it is run again, dismissed or its key lands

## What is decided

**Proposed** on lakelands#1502, for 0.21.0, in format 21; the operator
reviews it on the issue, and merging does not accept it. It extends
`docs/decisions/016-two-write-doors-by-origin.md`, whose stream ends at
a refusal unless the delivery asks to hold it;
`docs/decisions/058-a-source-may-be-forgotten.md`,
`docs/decisions/065-part-of-a-source-may-be-taken-back.md` and
`docs/decisions/039-a-source-is-retracted-as-one.md`, whose acts reach
a source's held records as they reach its landed ones; and
`docs/decisions/006-grants-not-roles.md`, whose sight a held record is
shown by. Each comes in force with the issue that builds it, and the
build rewrites those records where they stand.

A **held record** is the word for it, in the docs and in the API
(`held`, `record_held`). It is not a retention hold, the `restricted`
withholding of a stored record (006): the API says `hold` only as a
verb (`hold_refused`) and never as a noun, so the two do not meet.

### A delivery may ask that a record it is refused be held, and its stream goes on

`IngestOpts::hold_refused`, for one call, off by default. Without it
nothing changes: a refused record ends the stream, as 016 says. With
it, a record refused by a rule of the record is held, and the stream
goes on:

- **Held** are the refusals of the record checks and of the ingest's
  own record rules (016): the key's form, a field the door or the
  grants refuse, a value that does not fit, a reference to no live
  record, a required field left empty, `written_once`, a link of no
  relation or past its cardinality, and a record the writer's `only`
  does not admit.
- **Not held** are the refusals that are not the record's, and they end
  the stream as without the option: the lake, the context and the
  `ingest` grant; a `commit_window` out of bounds; a source forgotten,
  a key store not bound, a key missing; an outage. Nor is a record of a
  type the lake does not declare, or does not show its writer: a held
  record is kept against its type, and without one no rule says which
  of its cells are sensitive or erasable.
- **The window** is decided without the held record, as the host would
  resume at `from` without it. Every record after it is judged over the
  lake and the records before it in the stream, the held one left out,
  so the answer is the same wherever the store cuts. The window's append
  carries its written records and a `record_held` for each held one,
  and its `Committed` lists the stream `index` of each in `held`.
- **The refusal** goes into the held record and not into the stream:
  the one the writer would have been answered, kind, reason and names.

### A held record is keyed by its source, its delivery and its place in the stream

Its id is `(source, delivery, index)`: the record's source, the call's
delivery (the caller's, or the store's own, 016) and the record's
`index`, the place 016 already answers in a refusal. That is the file
or feed item and the row a host can name without the lake. A
delivery sent again under the same id meets its held records again: a
row refused the same way is a duplicate and appends nothing, and one
refused with other cells or another refusal is the next version of the
held record, under the same id. A host that wants a redelivery to find
its held records names the delivery; without one, each call is its
own.

### A held record keeps its cells as given, sealed as its landed values would be

**The cells** are the record's members as it sent them, each value in
its canonical form, never coerced to the field or judged against it:
the refusal may be that it does not fit. A host that maps a file's
columns hands its cells as text, and reads them back as text. The type,
the key as sent, `seen_at` and `upstream` are kept beside them.

**Sealing.** A cell of a field the type declares `erasable` is sealed
under the source's key, and under the delivery's apart key where the
delivery is sealed apart (065), in one box, as a `record_ingested`'s
members are (083); every other cell is plaintext, as a landed value of
that field would be. The box's nonce is of its own domain: the source,
the word `held`, the delivery, the index and the held record's version
before the event, so no record's box shares one, and the same cells
held again at the same version seal to the same bytes. A cell to seal
with no key store bound is refused `unbound`, and the stream ends, as
an erasable value of a landed record ends it.

**Sealing every cell**, as if every field were erasable, was weighed
and not taken. It would need a key store for any held record, and
would keep a refused cell better than the record keeps it once it
lands: the protection a host asked for is the type's, not more.

### `sensitive` and `erasable` apply to a cell as to the field it names

A cell is read as the field it names: followed through every rename
since, with the access the reader's grants give that field and
`sensitive`'s raise, as 006 states them. To a reader of the field the
cell is shown; to one to whom the field is masked, the cell is named
in `masked` and its value withheld; to one to whom the field is `none`,
the cell is not there, nor named. A cell that names no field of the
type, or a field a redeclaration has since removed, is read by the
type's default access, as a field no grant names; one whose field was
`sensitive` when it was held keeps the raise, so a redeclaration never
opens a cell the type withheld when it came. A cell sealed stays
sealed: `erasable` is never taken off a field (058).

**The refusal** is shown whole to a reader who is shown every name it
holds, and to any other as its kind alone, with `more`, so that a
refusal phrased to its writer's sight tells no other reader what is
hidden from it.

### A held record is worked by a holder of `ingest`, under the sight of its type

`Catalog::held(who, filter)` lists held records by source, type,
delivery and state, open or closed, in id order, paged by the lake's
limits, and gets one by its id. `Catalog::dismiss_held(who, id)` closes
one. Both need the `ingest` grant (`not_granted` otherwise). A held
record is there to its reader as a proposal's write is (006): not where
the reader is not shown its type, not at a key whose record is
restricted to a reader without `restricted`, and not, to a holder of
`only`, where the record its cells would leave is not admitted. A held
record not there is answered `not_found`, as one that never was.

**A new grant** was weighed and not taken: a held record is what an
ingest left, and who may feed the lake is who may see what it sent
back; each cell is already decided by the grants of its field, so a
new grant would decide nothing a field's grant does not. **The type's
reading grants alone** were not taken either: they would show every
reader of a type, an agent among them, rows the type refused, beside
its records, as if they were its values.

### A resubmission is a normal ingest of the same source that names what it resubmits

A host runs a held record again by ingesting the mended record under the
same source, with `resubmits: { delivery, index }` on its
`SourceRecord`. It is judged as any record of any ingest, by the
writer's grants, and lands, counts as a duplicate, or is refused. A
`resubmits` that names no open held record of the record's source
there to its writer is refused `not_found`, the same for a closed one
as for a hidden one. Resubmitted under `hold_refused` and refused
again, it is the next version of the same held record, its new cells
and refusal replacing the old.

The record names what it resubmits because the key may be what was
mended: a record refused for its key lands under another, and only the
id finds it.

### A held record closes by an event, in the append of what closed it

A held record closes, and each close is a `held_closed` event, naming
the id and why, in the same append as the write or the act that closed
it:

- `resubmitted`: its resubmission landed or counted as a duplicate.
- `landed`: a record write was accepted at its type and key after it
  was held, by any door: an ingest of any source, written or a
  duplicate; an applied `upsert`, `link`, verb or accepted proposal; a
  mirror's copy. A held record of a key no record could bear, an empty
  one, or one a re-keying migration has left in an old form, closes only
  otherwise.
- `dismissed`: its reader dismissed it. A repeat answers the first
  close's position and appends nothing.
- `forgotten`, `removed`, `purged`: its source was forgotten (058), the
  values its event wrote were taken back (065), or its source, or its
  type of that source, was purged (039). A `remove_values` selects a
  held record as it selects a `record_ingested`: by the position, the
  delivery and the `seen_at` of its last `record_held`.
A write that closes held records looks for them by type and key in the
projection, which keeps the open ones indexed so; a lake that holds
none of a type looks for nothing.

The event is there so that the close is in the log and the feed, which a
host follows, and the projection answers it too: a closed held record
is listed with its id, its close and why, and **no cell**. A close only
the fold inferred, with no event, would leave a host to reason from a
record event what a held record it may not know of did.

### What forgetting, taking back and purging do to a held record

Each act closes the held records it reaches in its own append, and its
preflight counts them per type. A forgotten source's sealed cells read
as erased in the log, as its landed values do, and no answer holds a
cell of it; a cell sealed under a destroyed version or apart key
(065) is shredded with it. A plaintext cell stays in the log, as a
landed plaintext value of the same field stays there after the act:
the act reaches the log through keys, and a held record is not
treated more harshly than a landed one. A dismiss closes and erases
nothing: the log keeps what was sent, as it keeps every write, and a
host that must erase a refused row's cells forgets or takes back its
source, as for a landed one.

### A held record lives until it closes, and a lake bounds how many it keeps

No clock closes a held record. It is held so that a person decides, and
a limit that dismissed it on a date would decide in their place, and
quietly. **A time limit** was weighed and not taken: a host that wants
one lists the held records older than its own limit and dismisses them,
and the log says who did.

What a lake bounds is how many it keeps: `limits.held_max`, the open
held records of one source, 10 000 by default, the host's to raise. A
held record costs a row of the projection while it is open, and a feed
gone wrong would otherwise fill the lake with rows it refused. A
refusal past the bound is not held: it ends the stream as without the
option, answered with its own kind and the reason `held_full`, naming
the bound.

**A redeclared type** keeps its held records, and a type is never
withdrawn (027), so no change of shape closes one. Its cells are read
under the shape at the head, through the type's and its fields' renames,
as above, and a resubmission is judged under it.

### Holding is asked per ingest, not set per source

The option is the delivery's, as `commit_window` is (016): only the
caller knows whether someone will work what it holds, and the option
changes the shape of the call's answer, a stream that ends at a refusal
or one that goes on. A **per-source setting** was weighed and not taken:
a caller would be answered by a setting it may not know of, and a
source would need a declared existence the lake does not give it. The
REST ingest takes `?hold_refused`, and a catalog that does not carry
the option to the store refuses it, `hold_refused_not_carried`, rather
than end a stream the caller asked to go on.

### What it takes to build

Estimated from the two acts on a source that shipped, as measured on
develop in the core crates: forgetting a source (lakelands#1042),
+1 535 product and +1 525 specification lines, and taking part of a
source back, its redaction step (lakelands#1153), +851 and +849. A held
record adds two event kinds, their fold and a projection table on three
engines, a change of the ingest's window, a sight of its own, sealing in
a new nonce domain, the closes in every record door and in three acts,
and two catalog methods with their REST doors:

- **Product:** about +1 500 to +2 000 lines. `IngestOpts::hold_refused`,
  `SourceRecord::resubmits`, `Committed::held`, the held record, its
  filter and its id in `lakelands-api`; the cells' sight in
  `lakelands-core` (`rights.rs`, beside a proposal's); the window, the
  closes, the acts' reach and the fold in `lakelands-store`; a table and
  two projection methods in each engine; `held`, `dismiss_held` and
  `?hold_refused` in the server and the client.
- **Specification:** about +1 600 to +2 200 lines, in the checks below.
- **Budgets:** the product has about 1 230 lines of room on develop
  today and the specification about 240, so the build cuts first and
  raises both by the rule of `docs/budgets.md`, with a measured row.
  Seam width rises by two, 91 to 93 of 110; `Catalog` is not counted.
- **Docs:** about +250 lines. 016's stream and order, 058's, 065's and
  039's acts, 006's sight, `docs/log-format.md` (two kinds in §2.3, the
  nonce domain in §1.4, the content id in §3, the step to 21 in §6),
  `docs/rest.md`, `docs/settings.md` (`limits.held_max`),
  `docs/refusals.md` (`held_full`, `hold_refused_not_carried`) and
  `docs/hosting.md`: name the delivery, and what a dismiss does not
  erase.
- **The log:** format 21, 0.21.0's one step: `record_held` (envelope
  `source` and `delivery`; payload the type, the key as sent, the id,
  `version`, the cells, `sealed`, `seen_at`, `upstream` and the
  refusal), content-addressed like an ingest so that a redelivery
  appends nothing, and `held_closed` (the id and why). A log of 20 holds
  neither; no converter changes an event.
- **WIT:** `hold-refused` on the ingest options and `resubmits` on a
  source record of `types.wit`. The listing and the dismiss stay off the
  guest door in 0.21.0: they are a person's, worked through the host.

The checks that would pin it, on every engine:
1. A delivery of three records, the second refused, with
   `hold_refused`: two land, one is held with its refusal, listed by
   source and type, and `Committed.held` names index 1. Without the
   option the stream ends at index 1, as before. The same answer at a
   `commit_window` of 1 and of 256.
2. Not held, and the stream ends: a type not declared, the `ingest`
   grant missing, a forgotten source, an erasable cell with no key store,
   and a refusal past `limits.held_max` (`held_full`).
3. Sight: a sensitive cell is masked to a reader without the field's
   right and shown with it; a `none` cell is not there; a held record of
   a type not shown, or of a restricted key, is not listed; without
   `ingest` the listing is `not_granted`; a refusal naming a hidden
   field is shown as its kind alone.
4. Sealing and acts: an erasable cell is sealed in the log and a
   verifier without the key sees no plaintext; `forget_source` closes
   every held record of the source and no cell of it is answered;
   `remove_values` by delivery, by time and by `seen_at` closes what it
   selects; a purge closes the source's; a rebuild and an import answer
   the same held records as the head.
5. Closing: a resubmission lands and closes `resubmitted`, under a
   mended key too; one refused again under `hold_refused` is version 2
   of the same id; an author's `upsert` and another source's ingest at
   the key close `landed`; a dismiss closes and its repeat appends
   nothing; a closed held record is listed with no cell.
6. Redelivery and redeclaration: the same delivery sent again appends
   nothing for a row refused the same way, and makes version 2 of one
   refused otherwise; a renamed field's cell is read under its new name;
   a renamed type's held records are listed under its new name; a
   removed sensitive field's cell stays masked.

## Why

A host that feeds a lake from files or feeds has rows the lake refuses,
and someone must look at them: run them again once the cause is
mended, or let them go. Until now the lake kept nothing of such a row
but its refusal, so the host kept the cells itself, and the cells are
the type's values: a copy beside the lake escapes the type's
`sensitive` and `erasable` and the source's key, and forgetting the
source does not reach it. Holding them in the lake puts them under the
rules that govern the values they would have become.

**In the log, not beside it.** The projection is the fold of the log
(001): a table beside the log would be a second truth that no rebuild
restores, no export carries and no verifier checks, and its erasure
would need a path of its own beside the keys 058 destroys. As events,
held records are rebuilt, exported, imported, sealed and erased by the
machinery that already does so for landed values, at the price of a
format step.

## What it is not

- **Not a proposal.** A proposal is a write its type accepts, waiting
  for a reviewer; a held record is one its type refused. Nothing
  accepts it: it lands only as a new write that passes every rule.
- **Not a record.** No read, view, set, trigger, search, index, derived
  field or constraint sees a held record, and it is answered only at the
  head, with no `as_of`: its history is in the log and the feed.
- **Not a repair.** The lake calls no model and guesses no value; it
  keeps what was sent and what it said, and a person or the host's own
  code mends it.
- **Not a query language.** The listing filters by source, type,
  delivery and state, and nothing else.
- **Not a retention hold** (006's `restricted`), and not an erasure: a
  dismiss closes, and only the acts on a source erase.
- **Not held** are the refusals that are not the record's, nor a record
  of a type the lake does not declare.
