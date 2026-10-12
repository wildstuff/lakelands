# 032 — A reference names a live record

## What is decided

A `reference` value a write carries must name a live record of the
field's target type: one at the head, one written earlier in the same
append or ingest window, or the record being written itself. A write that
names any other record is refused whole, on that field, and appends
nothing. The rule holds for every write door: `upsert`, a verb's write,
an accepted proposal and `ingest`. A verb that reads a field across a
reference reaches the far record by the same rule, at the call
(`docs/decisions/055-a-path-steps-through-a-reference.md`).

**A reference a record computes is held to the same rule.** A
`reference` field may be computed at the write, by an expression or an
`expr` function. The door judges the record as planned, not the patch:
each reference field that holds another value than the live record its
writer reaches held (a record written again after it was retired has
none), and each that is computed from a field the write sends, whatever
value it then holds, names a live record its writer is answered, as one
the write sends does (a record held from the writer or outside its
`only` is to it not there, 006 and 051, and so is every record of a
type it is not shown). A field sent again is a key
sent again, and that is judged though the record held it already; were
it not, a writer who may not read the reference would learn it by
guessing, a dead guess being applied exactly when it is the key held.
One the write neither moves nor sends a field of is not judged again,
as a sent one outside the patch is not: else a record whose reference
names a record retired since could not be written at all. A field is
sent for a reference that reads it directly or through other computed
fields; a written reference is computed from nothing, whatever it
lists beside it. At `ingest` a record its window sends wholly unchanged
is a duplicate and is not written, so nothing of it is judged, a sent
key as little as a computed one. A proposal's accept judges the record as the declarations in force then
compute it, and what a proposal reaches counts the records its computed
references name.

The refusal is the one a sent reference gets, made once the record is
computed and so after the version is compared: a stale
`expected_version` is told first, where an upsert tells a sent
reference first. To a writer who
may not read the reference field it is `not_granted` and names nothing:
the key is computed from the record's fields, which that writer may not
all read, where a sent key is the writer's own. Such a writer still
learns from each write whether the key its fields compute names a
record it is answered: judging the reference at all tells that much.
An accept judges the proposer's write as one who reads every field, so
this rule does not shape what an accept answers; what its reviewer is
told is the listing's rule (013).

**A reference nobody can judge is not declared.** One computed at the
read is never planned, and one computed by a function's backing lands in
the fold, which has no writer and refuses nothing. Declaring either is
refused as `reference_unjudged`, and so is declaring a function again
with a backing while a reference calls it or reads a field that does,
and a reference that reads a field a backing or an embedding computes,
directly or through other computed fields, since it lands in the fold
with that field. The refusal is the door's: the fold is not touched, and
a log that holds such a declaration folds as before.

Retiring a record that others refer to stays allowed. The references
then name a retired record, and a walk hides that end as it hides any
retired record (`docs/decisions/026-the-store-walks-the-graph.md`).

## Why

A relation carried by a reference field is only as good as the key it
holds. A dangling reference written today is a walk that silently finds
nothing tomorrow, and an agent cannot tell a missing partner from a
misspelt one. Checking at the door costs one lookup per reference, and
it catches the mistake where it is made.

Resolving within an ingest window lets a source deliver a partner and
the invoices that name it in one export, in order, without first waiting
for the partner to be committed.

## What it is not

- It is not referential integrity over time. A record may be retired
  while others still name it; the log keeps both, and history reads show
  what was true then.
- It does not order an ingest for the source. A reference to a record
  later in the same window is still refused.
- It does not keep a computed reference live after its write. Its
  target may be retired or held later, as a sent one's may.
- It does not judge a declaration. A reference is judged for its
  writer at a write. A declaration that changes what computes a
  reference (an expression or an `expr` function declared again, a
  type or a value type migrated) is its declarer's act, as retiring a
  record that others name is: the fold
  computes every record's reference again, with no writer and no
  judgement, and the records may then name what a writer could not
  have written. The next write of such a record that sends no field
  the reference is computed from is applied.

## How it came to be

- The first version: a reference a write carries names a live record.
- A reference a record computes is judged from the record as planned,
  where only the patch was read and a computed reference could name a
  record that was absent, retired or held; one computed at the read or
  by a function's backing is refused at declare (lakelands#795).
- A verb that reads across a reference reaches the far record by this
  rule (lakelands#567).
