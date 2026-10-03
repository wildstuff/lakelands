# 039 — A source is retracted as one

## What is decided

`LakeStore::purge_source(source)` retires every live record one source
brought in, across every type it reached, in one append, and answers what
it retired per type. Each retirement is an ordinary `record_retired`,
written by the store (`system:lakelands`, origin `system`), and reacts as
a `retire` would: triggers fire and object sets are left in the same
append. There is no new event kind. Restricted records go too, and so do
the link records the source brought in for a `source` relation.

Who brought a record in is read from its per-field provenance
(`docs/decisions/010-provenance-per-field.md`), over the fields that hold
a value. A record belongs to the purge when one of those came from the
source; a record whose values from the source were all cleared since
holds nothing of it, and stays. It is **co-owned**
when a field that holds a value came from another hand: a principal's
authored write, or another source. A derived value and a value the lake
wrote itself (a backed output, an aging move) make no owner, and a field
a person wrote and later cleared holds no value, so it co-owns nothing.
One co-owned record refuses the whole purge as invalid, naming the count
and the first ten, and nothing is appended.

A type declared without per-field provenance cannot say who brought its
records in. A purge does not search it and names it in its report:
opting out of provenance is opting out of purge.

## Why

A wrong export, a test feed, or a source the lake stops trusting is one
thing to take back, and the unit is the source, not the row or the type.
Retiring record by record makes a hundred decisions of one intent, can
stop halfway without saying so, and is written again by every host.
Retracting per type would leave the source's records in other types
behind, still referring to what went.

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
  `checkpoint` and `age`, with no tool.
- It is not erasure. Nothing leaves the log.
- It is not paged. The whole purge is one append, held in memory before
  it lands, under the writer's lock. A host that cannot afford that for a
  very large source retires through the doors, type by type.

## Extended: a purge may be read before it is made

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
  record the purge takes, or a link record one of whose ends it takes.
  A restricted record counts; the report is the host's. A retired
  record, a record the purge takes itself, and a relation derived from
  matching values, which stores nothing, do not.
- **A reference is reported, never a refusal.** A retraction a host owes
  is not held back by a record of another hand. A host with the stricter
  rule reads the report and does not purge.
- **`purge_preflight` answers the report and appends nothing.** It is
  refused as the purge would be, for a co-owned record, because both
  select through one function. Its report carries an id.
- **`purge_source` may repeat the id.** It is then refused as invalid if
  the lake has moved since, by any append, as a migration's apply is.
  The id names the source and the head: the report is a function of the
  two, so the same head means the same report. Without an id the purge
  is what it was, and its own report names what it left behind.

The reference pass reads each type that can refer into the purge once,
in key order, a page at a time: a type that declares a `via` relation
to a type the purge takes from, and the link type of a relation one of
whose ends it takes from. What it holds is the purged keys, a page and
ten references per relation. The selection itself still reads each
provenance type whole, as above.
