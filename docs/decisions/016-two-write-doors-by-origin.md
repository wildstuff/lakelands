# 016 — Two write doors, by origin

## What is decided

### A lake is written through two doors, and they stay two

`ingest` is how a source feeds the lake. It takes a stream of records,
each from the call's source or from the source the record names, and
in the call's delivery or in the delivery the record names (087), and
chunks it into group-commit windows, answering one
acknowledgement per window with the position it is durable at. It writes
only fields whose authority is `source` or `shared`. It is
content-addressed: an event's id is a hash of the source, the type, the
key, the record's version before the event and the values, so a record
delivered twice appends nothing the second time, and a value that
returns after a change is written again.

`upsert` (and every verb) is how an author writes. It takes one record's
sparse patch under a principal's grants, writes only fields whose
authority is `authored` or `shared`, and accepts an optional idempotency
key: a retry with the same key from the same principal answers the first
outcome and appends nothing. The key is recorded in the log.

The source's own offset belongs to the caller; the lake's position
belongs to the store. The two are never mixed.

### A field may be written by its type's verbs alone

A field declared `authority: verb` is written only by its type's
declared verbs, their `set`, called or accepted as a proposal of the
call, and by what the store writes as declared: the field's `initial`
(050) and an aging rule (036) (lakelands#1130). A lifecycle an operator
declares, its states and the verbs that move a record between them,
then holds at every door: each move passes the verb's `requires` and
the gate its risk puts in front of it (013). `upsert`, a proposed
`upsert`, `link`, `ingest` (and so a mirror's copy) and a host's
reaction, which is one of these, are refused the field. A component's
guest door calls the same doors. An aging rule may move such a field: the
rule is a transition the owner declared, and whoever may write
`sys.aging_rule` holds that right.

Since no create can send such a field, a constraint that reads only
fields its verbs write is judged at the declaration over their
`initial`s: one that breaks is refused `initial_breaks`, and a field with
no `initial` is judged as null and, where the constraint refuses that,
refused `no_initial_breaks`. A constraint with `at_ingest: admit` is
taken, since an ingest makes records whatever they hold, and so is one
that reads a field authors write too. A grant's `only` on such a field
admits a create where the field's `initial` is the value it names.

| door ↓ / authority → | `authored` | `source` | `shared` | `derived` | `verb` |
|---|---|---|---|---|---|
| `upsert`, `link`, an accepted `upsert` | ✓ | | ✓ | | refused |
| `ingest`, a mirror's copy | a source it `admits` | ✓ | ✓ | | refused |
| a declared verb's `set`, `initial`, aging | ✓ | | ✓ | | ✓ |

An authored field may name the sources it `admits` (069): `ingest`
writes it under such a source, and only where no other hand wrote it
last: no hand, its `initial`, or that source itself. Any other source,
and an admitted one over an author's or another source's write, is
refused the field as `wrong_door`, and its window appends nothing.

The refusal is `verbs_only`, naming the verbs that set the field which
its writer is shown and may call, so it knows the way; where it is shown
none it is `wrong_door`, as for a field of another authority, so a verb
kept from a writer is told to it by nothing (006). A writer kept from
the field is answered `unknown_field` first, as for a field the type
does not have. The event a verb writes stays `record_upserted`, caused
by its `verb_called` and carrying the verb as its procedure.

The declaration refuses `verb` where no record could be made: on a key
field, on a required field without an `initial`, and on a field the
fold computes; and a constraint that reads only such fields is judged
over their `initial`s, which no create can replace (`initial_breaks`).

### A type's records may be retired by its verbs alone

A type marked `retired_by: verbs` has its records retired only by its
declared verbs whose effect is `retire`, called or accepted as a
proposal of the call (013) (lakelands#1148). The lifecycle a field only
verbs write holds at every door for the record's state; this holds it
for the record's end: a host that declares "an order ends only by
`cancel` or `archive`" has each end pass the verb's `requires` and the
gate its risk puts in front of it. `retire`, a proposed `retire` and
its accept are refused, and so a host's reaction and a component's
guest door, which call the same doors, and a mirror's crossing of a
retirement (042). The default, `retired_by: authors`, is never
written.

What the store retires as the system stays: a purge of a source (039),
a forgetting (058), and the links of a relation a migration removes
(027). The mark speaks of who among the lake's writers ends a record,
not of the host's own acts on its sources and shapes. `unlink` stays
too: a link type is the store's, and a verb does not retire a link.
Nothing that reads the log judges a door again: an import, a rebuild
and an upgrade fold what was written.

The refusal is `verbs_only` with no field, naming the type's retire
verbs its writer is shown and may call; where it is shown none it is
`wrong_door`, alike whether such a verb is hidden or absent (006). It is
judged after `not_granted` and before the record, so a writer that may
not retire the type learns nothing new of it, and a key no record holds
is answered as a live one.

The declaration refuses the mark on a view, whose records are never
retired (`retired_by_view`), a retire verb on a relation's link type
(`retire_verb_link`) and one on its type (`type_verb_writes`). A type
marked with no retire verb is taken: a verb may come later, and
`describe` names the mark. The mark is not in the schema hash, so
marking a type, or taking the mark off, is a change of meaning (027).

### A window is decided whole, and a record that changes nothing is a duplicate

`ingest` takes its stream in group-commit windows: a window closes when it
holds the call's `commit_window` records, else the lake's
`commit_window_events`, or 2 ms after its first,
and is decided whole. A record whose content id is already in the log, or
earlier in its window, appends nothing; so does a record that would change
no field — a re-import of unchanged data appends nothing — and both count
as duplicates. A window with a refused record appends nothing; the
refusal is the stream's next item and the stream ends there, and the
caller's own offset says where to resume, unless the delivery asked
that the record be quarantined (below). The refusal says where that
is (lakelands#1043): it names the record, by the type and key its source
sent, its place in the stream (`index`, counted from 0 over every record
the stream handed out) and the place of the first record of its window
(`from`): every record before `from` is written, none from it on. An
outage names no record, since it is no record's. An authored `upsert` is
different on purpose: a write is a write, even with the same values.

**A window is the store's, and no answer turns on it** (lakelands#886).
The store cuts a stream where it chooses, and a host cannot make two
records share a window: over the wire they arrive as the socket brings
them. So no rule is judged over a window. A record is judged over the
lake and the records before it in its stream, as the window plans them,
and its refusal is the same wherever the store cut. What the cut decides
is only `from`: whether the records of the refused record's window
before it were written. The host's move is the same either way: resume
at `from` without the refused record. A record sent again lands with the
id it would have had, or counts as a duplicate where it was written.

**A delivery may ask the size of its windows, and the lake bounds it**
(lakelands#1496). Each window pays one durable append and one projection
commit, a cost that barely moves with the window's size, and a refused
record takes back its whole window. Which of the two matters more is a
property of the delivery, not of the lake: a supplier handing over a
thousand records it can send again whole wants them in one append, while
a feed of records arriving one by one wants a refusal to take back
little. Only the supplier knows which it is sending, so an ingest may
name `commit_window`, for that call alone; without it the lake's
`commit_window_events` applies, and a batch of record writes (073)
keeps the lake's value whatever an ingest asked. The lake bounds the
request with `limits.commit_window_max`, because a window is held in
memory whole while it is decided and a crash check on open reads the
last append whole: the host, not a supplier, decides what those may
cost. A window of 0 or above the bound is refused as `limit_range` on
`commit_window`, naming the bound, before anything is read, and a lake
whose own `commit_window_events` is above its bound is not opened. The
size changes only where the store cuts, which no answer turns on.

### A delivery may ask that a refused record be quarantined, and its stream go on

From lakelands#1502
(`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`):
an ingest may set `quarantine_refused`, for that call alone. A record
then refused by a rule of the record, every check of the order below
from the key's form on but a key holding `0x00`, an outage and a full
quarantine, is kept as a quarantined record with its cells and its
refusal, and the stream goes on. The window is decided without it, as
the host would resume at `from` without it, and its `Committed` lists
the stream `index` of each record it quarantined. A refusal of the
call, of the source, of the delivery's form or of a type not declared,
and one past
`limits.quarantined_max`, ends the stream as without the option; the
last carries `quarantine_full` beside `index`, `from` and `record`.

A record may name what it `resubmits`, a quarantined record of its
source by its first delivery and its place there. It is judged as any record; one
written or counted as a duplicate closes what it names, and a duplicate
that does so appends that close and nothing else: the one case where a
duplicate appends. A redelivery that names no open quarantined record
appends nothing, as before.

### A type may be written once by its source

A type marked `insert_only` takes each of its records from its source
once (lakelands#1044). Some sources deliver closed records: a finished
event, a settled measurement, a line of an append-only log. One that
comes back under its key with other values is a fault in the source, a
device restored from an old copy or a log rewritten, and must never
quietly replace what was stored. So an ingest that would write a key
the type already holds, live or retired, or that its window wrote
already, is refused as `written_once`, naming the record, and the
stream ends there as at any refusal. The same record again is a
duplicate, as on any type. A retired key stays written: re-making it
would be the rewrite the mark is there to stop.

The mark binds the source door alone. A type that sources its records
often carries fields its people write, a verdict or a note, and closing
those would forbid annotating a closed record; an author's write of an
authored field is taken as before. A key held from its writer, and
another's key to a holder of `only`, answer as on an unmarked type, so
the mark tells nothing of them. Declaring the mark on a type that holds
records, or taking it off, is a change of meaning
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`): no
record changes, and from that position the type's keys are written once
or freely.

### A record's key is checked before its fields, an empty record after the duplicate test

Within a window, a record's key form (not empty, no NUL character) is
checked before its fields, as `upsert` checks it. A record that carries
no field is judged after the duplicate test, on purpose. For a key the
lake holds live, it changes nothing, so it counts as a duplicate, as
any re-import that changes nothing does. For a key the lake does not
hold live, it is refused as invalid, as an empty `upsert` is.

### A source record may name where its values were read

A source record may name `upstream`, the other lake and position its
values were read from (`docs/decisions/010-provenance-per-field.md`,
extended). The ingest door takes it from the ingesting principal as it
takes `seen_at`, and no grant decides it. It is not in the event's id,
so a window delivered again stays one event whatever it names, and no
duplicate rule changes (lakelands#174).

### The fold writes one value that no door admitted: a field's initial

The authority line holds for what an event carries. One value crosses
it where the fold applies a write, not in the event: a field that
declares `initial` is filled when its record is made, also by an ingest
that may not carry the field
(`docs/decisions/050-a-field-declares-its-initial-value.md`). The event
stays as the door wrote it, so the content id and the duplicate test
read the same patch, and a delivery of the same content is still a
duplicate. A door judges `required` over the record as the fold will
leave it, filled values included.

### The order of refusals is part of the contract

When a write breaks more than one rule, the caller is told the first
the store checks, and a host or a model acts on that answer. So the
order is part of the contract, stated here for the record writes and
in the record of each other door, and held by the conformance suite: a
request that breaks two adjacent rules is answered with the earlier.
Where the two refusals cannot be told apart by kind, field and the
names a refusal must mention (015, extended), the order is stated and
not checked.

**Every write door** opens and closes the same way:

1. **The lake.** A principal of another lake is `not_granted`.
2. **The context.** A depth above the lake's ceiling is `too_deep`
   (021, extended). It reads nothing and comes before the writer is
   taken.
3. *The door's own checks, in the order below.* A door that takes an
   idempotency key answers a retry with the first write's outcome where
   its order names "the retry"; nothing it checks after that point can
   refuse a retry.
4. **The append**, under the writer: a handle opened read-only is
   `unavailable`; a host's cause past the head is `invalid`; a cause
   that names a firing is `too_deep` when the depth is above that
   trigger's ceiling (021, extended).

**The record checks** decide every record a write makes, by `upsert`, a
verb's effect, a proposal and its acceptance, and `ingest`:

1. the type is declared: `not_found`;
2. the patch is not empty: `invalid`;
3. the key is not empty and holds no NUL: `invalid`;
4. the door and the grants: a view is `invalid` (018, extended); then
   field by field, in name order, a field the type does not declare is
   `invalid`, one another authority writes is `wrong_door`, or
   `verbs_only` where only its type's verbs write it and its writer is
   shown one, and one the principal may not write is `not_granted`, each
   naming the field, so the first offending field by name is the one
   named;
5. every value fits its field, in name order: `constraint`, naming it;
6. on a create, the key fields the patch leaves out are filled from the
   key: a key that does not split into them is `invalid`; a filled
   field is granted and fits as in 4 and 5;
7. the key and the key fields agree: `invalid`;
8. every reference the write sends names a live record, in name order:
   `constraint`; a reference the record computes is judged once the
   record is computed, after 11, so a stale version is told first
   (`docs/decisions/032-a-reference-names-a-live-record.md`);
9. the expected version: `conflict`;
10. every required field holds a value after the write: `constraint`;
11. a `sys.aging_rule` record, or a `sys.setting` record of a setting
    the store reads itself (`sort_index`, `vector_index`,
    `search_weights`, `view_kept`), is well formed: `invalid`, naming
    the field;
12. the record is the principal's to write: `not_granted`, naming no
    field, where to a holder of `only` another's live record stands at
    the key or the record the write leaves is not admitted (051), and
    where to a writer without the `restricted` grant a restricted
    record, live or retired, stands at the key (006).

What the request says on its own comes first (1 to 5), then what it
says against the head (6 to 9), then the record it would make (10 to
12). Steps 2 and 3, and 6 and 7, answer the same kind with no field, so
their order is stated, not checked.

**`upsert`**: the lake, the context, a link type at the ordinary door
(`wrong_door`), the retry, the record checks, the append.

**`retire`**: the lake, the context, a link type at the ordinary door
(`wrong_door`), the retry, the type is declared (`not_found`), it is
not a view (`invalid`), the principal may write every field of it
(`not_granted`), the type is retired by its authors (`verbs_only`, or
`wrong_door`), the record is live (`not_found`), the expected version
(`conflict`), the append.

**`ingest`**: the lake, the context and the `ingest` grant
(`not_granted`), when it is called, before it takes the stream; each
window checks the context again. Then each record, in stream order: the
type is declared (`not_found`); the key's form (`invalid`); the door and
the grants, field by field (record check 4); the form of a source and
then of a delivery the record names in place of the call's (`invalid`,
`source_id_form`, `delivery_id_form`); every value fits (5); a
record of a link type no relation carries, its relation renamed or
removed, is `invalid`, naming the type (lakelands#761); then the
duplicate test, which counts a record and checks nothing further; then
the record checks from the empty patch on, the expected version
excepted; then a link of a relation carried by link records holds its
cardinality, once the record its writer leaves is admitted (051): a
link from its `from` is looked for over the lake's live links and then
the window's earlier ones, and only then a link to its `to`, in the
same order (`constraint`, naming `to`). The refusal is
`linked_already_from` or `linked_already_to`, naming the other link's
end by one rule. The link found does not depend on which side of the
cut it stands, so the cut changes no answer, `one_to_one` included.
Under `quarantine_refused`, a record refused at any of these checks
from the key's form on is quarantined unless the quarantine of its
source is full (085), and a record named by `resubmits` that names no
open quarantined record there to its writer is `not_found`.

### A door decides over everything the log holds

An append is durable before it is folded, and a fold can fail: the
events are in the log and the projection, which every check above
reads, does not hold them. A write decided there would answer a retry
as a new write and give a record a version it already has. So the
writer is taken for a write in one place, and a writer whose fold
failed folds from the projection's position there, before the door's
own checks; where that fold fails the write is `unavailable`, whatever
kind the fold answered, and appends nothing. The store's own writes
take the writer there too: the drain of due calls reads what it decides
on behind the catch-up. An append decided under a writer that is behind
was decided some other way: it is refused before it reaches the log,
and appends nothing. The write whose fold failed is answered
`unavailable` with its position in the log: sent again under its
idempotency key it is the retry, answered the first write's outcome;
without a key it is a second write. No kind of its own tells it from a
write that never landed: a caller without a key cannot know that of any
`unavailable` write, so the kind would promise what a door cannot keep.

## Why

The two doors are where the field-authority line runs. A re-import must
never overwrite a verdict a person wrote, and an author must never
overwrite a figure the source is the authority on. Keeping the doors
separate makes that line visible in every call: the name of the door
says which side of it the write is on, and the store checks the fields
against it.

Idempotency follows from origin too. A source redelivers the same
content, so content is the right key. An author may write the same
content twice on purpose, so an author's retry needs a key of its own,
chosen by the caller.

Taking a stream from the first signature means a real streaming source
is just another caller later, not a change to the contract.

## What it is not

- It is not two stores. Both doors append to the same log and are folded
  by the same rules.
- `ingest` offers no exactly-once promise and needs none: at-least-once
  delivery of the same content is a no-op.
- A `shared` field may be written through either door; the later
  position wins.

## How it came to be

- The first version: two doors by origin, `ingest` content-addressed
  over a stream, `upsert` with an idempotency key.
- Windows, duplicates and refusals were stated: a window is decided
  whole, a record that changes no field counts as a duplicate, and the
  content id takes in the record's version before the event.
- A source record may name `upstream` (lakelands#174).
- Within a window, the key's form moved before the fields and the empty
  record behind the duplicate test (lakelands#344).
- The order of refusals was stated for the record writes and pinned in
  the suite (lakelands#338).
- A writer that is behind catches up before a write is decided
  (lakelands#603).
- A field's `initial` is filled by the fold, also in a record an ingest
  makes (lakelands#551).
- The order of `ingest` took in the refusal of a record of a link type
  no relation carries, before the duplicate test, and a link's
  cardinality moved from before the duplicate test to behind the record
  checks (lakelands#761).
- Corrected at the rewrite (lakelands#817): `upsert` was said to write
  "only authored fields", against the `shared` field of "What it is
  not"; and the record checks named the type of an aging rule
  `sys.aging`, which is `sys.aging_rule`.
- Corrected at the rewrite (lakelands#817): record check 11 named only
  a `sys.setting` index setting. The store judges every setting it
  reads itself there, `search_weights` and `view_kept` too.
- Corrected at the rewrite (lakelands#817): the record checks ended at
  11 and held no step for the write a holder of `only`, or a writer
  without `restricted`, is not granted. The store refuses it last,
  once every other record check has passed, and the suite holds that a
  probe answers as on a free key before it.
- A refused record names itself and where its window began, so the
  caller's offset has a value: `index`, `from` and `record` beside the
  refusal (lakelands#1043).
- A type may be marked `insert_only`: its source writes each record
  once, a retired key included, and an author's fields stay writable
  (lakelands#1044).
- One ingest may carry many sources: a record may name its own `source`, which its event and its id take in place of the call's (lakelands#1041).
- A source id is formed as a key is, not empty and holding no NUL, and a source record with a member it does not have is refused, never read without it (lakelands#1081).
- A window is the store's: no rule is judged over one, and a link that breaks its relation's cardinality with an earlier link of its window is answered as over the lake, the other end named by the same rule; `window_links_already` is never answered from 0.15.0 (lakelands#886).
- A field may be written by its type's verbs alone, `authority: verb`; the other doors are refused it as `verbs_only`, naming the verbs its writer is shown, or `wrong_door` (lakelands#1130).
- An authored field may admit named sources, `admits`, which `ingest` writes it under where no other hand's value stands (069, lakelands#1264).
- A type's records may be retired by its verbs alone, `retired_by: verbs`; `retire`, a proposed `retire` and its accept are refused as `verbs_only`, naming the retire verbs its writer is shown, or `wrong_door`, and the system's retirements stay (lakelands#1148).
- An ingest may ask its own commit window, `commit_window`, up to the lake's `limits.commit_window_max`; record writes sharing one append keep the lake's `commit_window_events` (lakelands#1496).
- One ingest may carry many deliveries: a record may name its own `delivery`, which its event takes in place of the call's, its form judged as the call's beside the record's source form (087, lakelands#1528).
