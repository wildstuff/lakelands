# 039 — A source is retracted as one

## What is decided

### A purge retires every live record a source brought in, in one append

099 appends a purge in parts, between `source_purged` and `act_whole`;
the rule below of one append and the note that it is not paged change
with it.

`LakeStore::purge_source(source)` retires every live record one source
brought in, across every type it reached, in one append, and answers what
it retired per type. Each retirement is an ordinary `record_retired`,
written by the store (`system:lakelands`, origin `system`), and reacts as
a `retire` would: triggers fire and object sets are left in the same
append. There is no new event kind. Restricted records go too, and so do
the link records the source brought in for a `source` relation.

### Provenance says what a purge takes, and one co-owned record refuses it whole

Who brought a record in is read from its per-field provenance
(`docs/decisions/010-provenance-per-field.md`), over the fields that hold
a value. A record belongs to the purge when one of those came from the
source; a record whose values from the source were all cleared since
holds nothing of it, and stays. It is **co-owned** when another hand
made it, or when a field that holds a value came from another hand: a
principal's authored write, or another source. A derived value and a
value the lake wrote itself (a backed output, an aging move) make no
owner, and a field a person wrote and later cleared holds no value, so
it co-owns nothing.
One co-owned record refuses the whole purge as invalid, naming the count
and the first ten, and nothing is appended.

Who made a record is read from the write at its `made` position: the
source made it when that write was the source's ingest. A record another
hand made is never retired by taking a source back, whatever its fields
hold now; the purge refuses it as co-owned, removing values erases the
source's values on it and keeps it (065), and forgetting keeps it (058).
One test, at the `made` position, answers for all three.

On a record the source made, a field that declares `initial` co-owns
nothing, whoever wrote it last
(`docs/decisions/050-a-field-declares-its-initial-value.md`). It is
there because its record is, and the record is there because the source
brought it: an author who moved the state did not bring the record, and
when the source is taken back the state goes with it. This rule does
not decide whether a record is retired: a record another hand made
stays whatever its fields hold, such a field's value alone or its
`initial` included. A whole purge and a typed one ask the same rule.
The report says what went: `moved` holds, per type and field, how many
records the purge takes whose such field an author had written, and the
first ten by key; it is left out when empty, and the preflight says the
same. Every other authored field co-owns, so a record where a person
wrote anything else still refuses the purge whole.

A type declared without per-field provenance cannot say who brought its
records in. The purge of a source whole does not search it and names
it in its report, and a purge of that type alone is refused: opting out
of provenance is opting out of purge.

### A purge may be read before it is made

A purge leaves behind every record of another hand that named a record
it took: a bill whose `partner_id` holds a purged partner's key, another
source's link at a purged end. The purge is right (a source is retracted
as one) and the walk is right (a link to a retired end is no link, 026),
but nothing told the host. A host whose own rule forbids such a
retraction had to rebuild the purge's selection to hold it, or refuse
every retraction (lakelands#473).

- **The report names what the purge strands.** Per relation, in type
  then relation order: how many live records outside the purge refer to
  a record inside it, and the first ten in key order, by type and key
  only. A reference is a relation's `via` field that holds the key of a
  record the purge takes, alone or as any element of an array of keys
  (`docs/decisions/091-a-field-carried-relation-over-an-array-of-keys-yields-one-edge-per-key.md`), or a link record one of whose ends it takes. A
  record is counted once under a relation, however many of its keys the
  purge takes.
  A restricted record counts; the report is the host's. A retired
  record, a record the purge takes itself, and a relation derived from
  matching values, which stores nothing, do not.
- **A reference is reported, never a refusal.** A retraction a host owes
  is not held back by a record of another hand. A host with the stricter
  rule reads the report and does not purge.
- **`purge_preflight` answers the report and appends nothing.** It is
  refused as the purge would be, for a co-owned record, because both
  select through one function. Its report carries an id, and how many
  memberships and firings the retirements would set off across an edge
  (`set_off`) beside the lake's `max_set_changes`. It reads, on any
  handle, and takes no lease.
- **A purge past the ceiling is refused whole.** Past `max_set_changes`
  of what its retirements set off across an edge
  (`docs/decisions/055-a-path-steps-through-a-reference.md`) the purge
  is refused `too_large`, naming the set, and appends nothing, as one
  co-owned record refuses it whole: a host learns at the preflight that
  it must narrow the set or open the lake with a higher ceiling.
- **`purge_source` may repeat the id.** It is then refused as invalid if
  the lake has moved since, by any append but a `lease_taken` or a
  `checkpoint`. The id is of the source, the type and the head, the
  log's last event that is neither: the report is a function of the
  three, so the same head means the same report. Only the position the
  report names may differ, as it is the log's last event when read,
  lease or not. An id of the source and the head alone would let the
  report of the source whole authorise a purge from one type at the same
  head, and the reverse. An id that does not fit is refused as invalid
  in one sentence for both causes: the store cannot tell a moved lake
  from another purge's id. Without an id the purge is what it was, and
  its own report names what it left behind.
- **A lease does not move the head** (lakelands#1469). Each process
  that opens the lake for writing appends a `lease_taken` before it
  acts, and may seal the segment it ends with a `checkpoint`; neither
  moves a record, key or declaration a report is made from. Were they
  counted, an operator's preflight and act, run as two commands, would
  never meet at one head. Every other append, a record of any source or
  a declaration, moves it. A forgetting's id (058) and a removal's (065)
  name the head the same way.

The reference pass reads each type that can refer into the purge once,
in key order, a page at a time: a type that declares a `via` relation
to a type the purge takes from, and the link type of a relation one of
whose ends it takes from. What it holds is the purged keys, a page and
ten references per relation. The selection itself still reads each
provenance type whole.

### A purge may name one type

A host that wanted one type of a source back had no way to say so. The
purge read a source and a type written together as a source nobody fed,
and answered that nothing was there; and one co-owned record in any
type of the source held back the records of every other
(lakelands#562).

- **The purge and its preflight take one optional type.** Without it
  the purge is the one above, to the letter. With it, the purge is of
  what the source brought into that type. One type, not a list: two
  types are two purges, each one append with its own report.
- **The named type's own links go with it.** The selection reads the
  named type and the link types of the relations that type declares: a
  record's own links are nothing without the record, and a host does
  not spell the store's name for a link type. A link of another type's
  relation that ends at a record the purge took stays, and the report
  names it.
- **Co-ownership is judged in the selection alone.** A co-owned record
  in the named type refuses the purge whole; one in another type of the
  source does not hold it. What is never split is the record, and no
  record is.
- **A type that cannot be purged from is refused.** A name that is no
  declared type is not found. A view, and a type declared without
  per-field provenance, are refused as invalid, saying why. The purge
  of a source whole passes over both, and names the type without
  provenance as not searched (a view holds no records, and is not
  named); a host that asked for exactly that type would read "nothing
  retired" as "nothing there". A typed report names no type as not
  searched.
- **The report names its type, and what it left behind.** The
  references include the source's own records in its other types that
  still refer into what went.
- **No event of its own.** A typed purge appends fewer of the same
  `record_retired`; nothing is replayed differently.

A host that must take back part of a source, values and not records,
co-owned ones included, has its own act
(`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).

### A purge closes the source's quarantined records

From lakelands#1502 (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`):
a purge closes every open quarantined record of its source, or of the
source and the type it names, `purged`, in its own append, and its
preflight and report count them per type (`quarantined`). A purge
erases nothing, so their cells stay in the log as its records' values
do; a closed quarantined record is listed with no cell.

## Why

A wrong export, a test feed, or a source the lake stops trusting is one
thing to take back, and the unit is the source, not the row or the type.
Retiring record by record makes a hundred decisions of one intent, can
stop halfway without saying so, and is written again by every host.
Retracting per type would leave the source's records in other types
behind, still referring to what went.

That holds for the purge without a type, which stays the way to take a
source back. The report says what a purge strands, so a host that takes
one type back is told which of the source's other records still point
there, and decides.

Refusing a co-owned record whole is what keeps the act honest: removing
it would take a person's or another source's values with it, and
skipping it would leave a count that says less than it did. The host
decides such records by the ordinary doors and purges again.

Retiring rather than erasing keeps the log the truth: a read as of
before the purge still sees the feed, and the purge is in the log with
its writer. The same content ingested again after a purge is a new
ingest, because the ingest id carries the version it was written over
(`docs/log-format.md` §3).

## What it is not

- It is not a door. An agent that wants data retracted proposes
  retirements. The purge is the host's act, beside `rebuild`,
  `checkpoint` and `age`, with no tool. A purge that names a type is
  still not a door.
- It is not erasure. Nothing leaves the log.
- It is not paged. The whole purge is one append, held in memory before
  it lands, under the writer's lock. A host that cannot afford that for a
  very large source retires through the doors, type by type.

## How it came to be

- The first version: `purge_source` retires what one source brought in,
  in one append, and one co-owned record refuses it whole.
- The report names what a purge strands, and `purge_preflight` answers
  it before the purge, under an id the purge may repeat (lakelands#473).
- A purge may name one type: co-ownership is judged in the selection
  alone, the report names the source's own records left behind, and the
  id takes in the type (lakelands#562).
- A field that declares `initial` makes no owner, and the report says
  in `moved` what an author had written there (lakelands#551).
- A record another hand made, an author or another source, is
  co-owned, read at its `made` position by the purge, forgetting and
  removing values alike: an author's record whose only author value was
  an `initial` is no longer retired by the first two while the third
  kept it, nor one whose every value the source wrote over
  (lakelands#1278).
- Corrected at the rewrite (lakelands#817): every purge was said to
  name a type it does not search, where only the purge of a source
  whole does; and a view was said to be named, where it is passed over.
- Extended by 091 (lakelands#1551): a `via` field that holds an array
  of keys refers to the record of each key in it, and its record is
  counted once.
- Amended by 099 (lakelands#1652), proposed for 0.23.0.
