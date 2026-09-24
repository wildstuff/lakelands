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
