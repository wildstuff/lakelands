# 042 — Water crosses only as a mirror

## What is decided

A lake is one world with one truth, and nothing reads across lakes.
Types cross from one lake into another only as a **mirror**: a host
component that follows the source lake's change feed and ingests the
records of the types a connection maps into the target lake, with their
provenance. The connection's principal reads the source and writes the
target, so grants decide what crosses on both sides.

1. **The name is the target's, and a connection is an explicit map.**
   Each connection maps a source type to a target type, the same name
   unless it says otherwise. A target type has at most one source; a
   second connection onto it is refused at start, naming both. Two
   sources of one kind land as two types. The mirror never merges.
2. **A mirrored type belongs whole to its source.** The target type
   declares only fields that cross, every one of them a `source` field
   nobody in the target can author. The target declaration is the list
   of what crosses. The target lake's own data about a mirrored record
   lives in a type of its own that refers to the copy, so
   `purge_source` of the connection removes the copies whole
   (`docs/decisions/039-a-source-is-retracted-as-one.md`).
3. **The source position rides with the data.** Every value a mirror
   delivers carries `upstream`, the source lake and the position of the
   state it reflects, per field beside `seen_at` (log format version 3,
   `docs/decisions/010-provenance-per-field.md` and
   `docs/decisions/016-two-write-doors-by-origin.md`, extended). A read
   in the target says which state of the source it holds.
4. **It fails closed, and visibly, by type.** A source declaration the
   target's declaration no longer admits (another key rule, a field gone
   or retyped, a re-key) stops that type of the connection. Its state,
   the position reached and why it stopped, is a `sys.mirror` record in
   the target, one per connection and source type, readable through the
   catalog. Nothing is dropped silently; a new source field crosses only
   once the target declares it.
5. **One home lake per field.** A field is a source field in one
   direction only. A connection whose target type is the source type of
   a connection the other way is refused, naming both.
6. **One writer.** The mirror writes through the target's writer. A
   target this process does not hold as its writer is refused at start,
   naming the lease (`docs/decisions/008-one-writer-per-lake.md`).

Two further rules follow from the tree:

- **What crosses with its targets.** A reference field of a mirrored
  type crosses only when the same connection mirrors the type it refers
  to; a link type crosses only with both of its ends
  (`docs/decisions/035-a-link-is-a-record.md`).
- **What is not ingest.** A retirement crosses through `retire`, and a
  retention hold through `restrict` only when the principal holds the
  `restricted` grant. What the principal cannot see, a withheld record or
  a hold without that grant, does not cross, and the copy keeps its last
  state.

The mirror reads the feed for order and position only. The feed is the
log unmasked, not filtered per principal
(`docs/decisions/037-the-feed-over-a-network-door.md`), so values are
read through the doors, as the principal, at the position the feed
names. Delivery is at least once, in
order, and a window delivered again appends nothing: ingest is
content-addressed.

## Why

One truth per lake is what makes every answer in it checkable against
its own log. A query that read another lake, or a reference that pointed
into one, would make an answer depend on a state the lake does not hold
and cannot verify, and a lake would no longer be whole when it is
exported, imported or moved to another engine. Copying the types a lake
needs, with the provenance that says where and from which state each
value came, keeps every lake whole and lets a reader follow a value back.

Making the copy belong wholly to its source keeps the two authorities
apart: the source lake decides the copied fields, the target lake decides
everything it says about them, in types of its own. A purge, a stop or a
redeclaration then touches one side only.

Failing closed by type, with the state in the target's own record, is
what makes a mirror safe to run unattended: a change the target does not
admit stops only that type, says why where an operator reads it, and
resumes from the position it stopped at once the target admits it.

## What it is not

- **No query across lakes, and no reference into another lake.** A read
  in one lake never reads another.
- **No two homes for a field**, and no merge of two sources into one type:
  a view over two copies is a view's business.
- **Not replication.** Only the mapped types cross, and only what the
  principal may read. The copy's history steps once per window, not once
  per source event, and a mirror copies what the source holds when it
  starts rather than replaying the source's history.
- **No network a host did not configure.** The source is a lake of the
  same root; a source served elsewhere is a later step.
- **Not a store operation.** The store has no notion of a connection. It
  holds the `upstream` member and the `sys.mirror` records the host's
  vocabulary declares, and the mirror uses the doors every host has.

## Extended: a copy keeps its chain's depth (lakelands#420)

A copy was written at depth 0, with origin `direct` and no cause, so
every crossing started a chain of reactions anew. Take two lakes that
mirror two types into each other, x one way and y the other, with a
host on each side that reacts to one by writing the other. Their chain
never met either lake's `max_reactive_depth`, since each crossing reset
it, and it ran without end.

A copy is now a reaction to the source's write. It is written with
origin `reactive`, at the depth of the source state it copies:
- a window crosses at the depth of its deepest change;
- a retirement crosses at the depth of its own event.

It names no cause, since the cause is an event of another lake;
`upstream` already names the source lake and position. A chain then
keeps growing across the lakes. The target refuses a copy deeper than
its own ceiling as `too_deep`, and that stops the type, as any refused
window does. The mirror's output says which record crossed at which
depth and the target's ceiling, so the chain ends at the crossing that
reached the ceiling, in plain sight.

A snapshot copies what the source holds at its head, a standing state
and no chain in flight, so it still crosses at depth 0.
