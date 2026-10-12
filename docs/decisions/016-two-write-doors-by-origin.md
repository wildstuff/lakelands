# 016 — Two write doors, by origin

## What is decided

A lake is written through two doors, and they stay two.

`ingest` is how a source feeds the lake. It takes a stream of records for
one source and chunks it into group-commit windows, answering one
acknowledgement per window with the position it is durable at. It writes
only fields whose authority is `source` or `shared`. It is content-addressed: an
event's id is a hash of the source, the type, the key and the values, so
a record delivered twice appends nothing the second time.

`upsert` (and every verb) is how an author writes. It takes one record's
sparse patch under a principal's grants, writes only authored fields,
and accepts an optional idempotency key: a retry with the same key from
the same principal answers the first outcome and appends nothing. The
key is recorded in the log.

The source's own offset belongs to the caller; the lake's position
belongs to the store. The two are never mixed.

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

## Extended: windows, duplicates and refusals

`ingest` takes its stream in group-commit windows: a window closes when it
holds the lake's `commit_window_events` records or 2 ms after its first,
and is decided whole. A record whose content id is already in the log, or
earlier in its window, appends nothing; so does a record that would change
no field — a re-import of unchanged data appends nothing — and both count
as duplicates. The content id includes the record's version before the
event, so a value that returns after a change is written again. A window
with a refused record appends nothing; the refusal is the stream's next
item and the stream ends there, and the caller's own offset says where to
resume. An authored `upsert` is different on purpose: a write is a write,
even with the same values.

## Extended: where a source read its values

A source record may name `upstream`, the other lake and position its
values were read from (`docs/decisions/010-provenance-per-field.md`,
extended). The ingest door takes it from the ingesting principal as it
takes `seen_at`, and no grant decides it. It is not in the event's id,
so a window delivered again stays one event whatever it names, and no
duplicate rule changes (lakelands#174).

## Extended: a record's key first, an empty record after the duplicate test (lakelands#344)

Within a window, a record's key form (not empty, no NUL character) is
checked before its fields, as `upsert` checks it. A record that carries
no field is judged after the duplicate test, on purpose. For a key the
lake holds live, it changes nothing, so it counts as a duplicate, as
any re-import that changes nothing does. For a key the lake does not
hold live, it is refused as invalid, as an empty `upsert` is.

## Extended: the order of refusals (lakelands#338)

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

**A door decides over everything the log holds.** An append is durable
before it is folded, and a fold can fail: the events are in the log and
the projection, which every check above reads, does not hold them. A
write decided there would answer a retry as a new write and give a
record a version it already has. So the writer is taken for a write in
one place, and a writer whose fold failed folds from the projection's
position there, before the door's own checks; where that fold fails the
write is `unavailable`, whatever kind the fold answered, and appends
nothing. The store's own writes take the writer there too: the drain of
due calls reads what it decides on behind the catch-up. An append
decided under a writer that is behind was decided some other way: it is
refused before it reaches the log, and appends nothing. The write whose fold failed is answered `unavailable` with its
position in the log: sent again under its idempotency key it is the
retry, answered the first write's outcome; without a key it is a second
write. No kind of its own tells it from a write that never landed: a
caller without a key cannot know that of any `unavailable` write, so
the kind would promise what a door cannot keep.

**The record checks** decide every record a write makes, by `upsert`, a
verb's effect, a proposal and its acceptance, and `ingest`:

1. the type is declared: `not_found`;
2. the patch is not empty: `invalid`;
3. the key is not empty and holds no NUL: `invalid`;
4. the door and the grants: a view is `invalid` (018, extended); then
   field by field, in name order, a field the type does not declare is
   `invalid`, one another authority writes is `wrong_door`, and one the
   principal may not write is `not_granted`, each naming the field, so
   the first offending field by name is the one named;
5. every value fits its field, in name order: `constraint`, naming it;
6. on a create, the key fields the patch leaves out are filled from the
   key: a key that does not split into them is `invalid`; a filled
   field is granted and fits as in 4 and 5;
7. the key and the key fields agree: `invalid`;
8. every reference names a live record, in name order: `constraint`;
9. the expected version: `conflict`;
10. every required field holds a value after the write: `constraint`;
11. a `sys.aging` rule or a `sys.setting` index setting is well formed:
    `invalid`, naming the field.

What the request says on its own comes first (1 to 5), then what it
says against the head (6 to 9), then the record it would make (10 and
11). Steps 2 and 3, and 6 and 7, answer the same kind with no field, so
their order is stated, not checked.

**`upsert`**: the lake, the context, a link type at the ordinary door
(`wrong_door`), the retry, the record checks, the append.

**`retire`**: the lake, the context, a link type at the ordinary door
(`wrong_door`), the retry, the type is declared (`not_found`), it is
not a view (`invalid`), the principal may write every field of it
(`not_granted`), the record is live (`not_found`), the expected version
(`conflict`), the append.

**`ingest`**: the lake, the context and the `ingest` grant
(`not_granted`), when it is called, before it takes the stream; each
window checks the context again. Then each record, in stream order: the
type is declared (`not_found`); the key's form (`invalid`); the door and
the grants, field by field (record check 4); every value fits (5); a
link of a relation carried by link records holds its cardinality over
the lake's live links, then over the window's earlier links
(`constraint`, naming `to`); then the duplicate test, which counts a
record and checks nothing further; then the record checks from the
empty patch on, the expected version excepted. The two cardinality
steps answer alike, and are stated, not checked.

## Extended: the one value the fold writes that no door admitted

The authority line holds for what an event carries. One value crosses
it where the fold applies a write, not in the event: a field that
declares `initial` is filled when its record is made, also by an ingest
that may not carry the field
(`docs/decisions/050-a-field-declares-its-initial-value.md`). The event
stays as the door wrote it, so the content id and the duplicate test
read the same patch as before, and a delivery of the same content is
still a duplicate. A door judges `required` over the record as the fold
will leave it, filled values included.
