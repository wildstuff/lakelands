# 009 — Every response carries the position it was read at

## What is decided

### An answer carries its position, and a read may be pinned to one

Every answer the store gives, to a read or a write, carries the
`position`: the log coordinate the answer reflects. A read can be
pinned to an earlier position with `as_of`, and a session (an MCP
session, for instance) can pin one `as_of` for a whole turn so that
every read in it sees the same state.

Business time is a separate axis. `valid_at` asks what was true in the
world at a moment; `as_of` asks what the lake knew at a position. The
two combine, and neither stands in for the other.

Over MCP, the session pin is the generated `pin` tool: it pins `as_of`
for the session's later reads until cleared, a read's own `as_of` still
wins, and writes are never pinned.

### An earlier position is served from kept history

A read at an earlier position is served from history the projection
keeps, not by replaying the log. Every state the fold writes is kept
with the position it became valid at; a view at a position filters to
the states valid there. This makes "what did the agent see" as cheap as
a current read on every engine, which is what makes it usable for audit
rather than a special operation. The engine seam states it
(`docs/seams.md`), and the conformance suite compares every as-of read
with a replay to that position.

### A version from before a retirement never applies

Compare-and-set compares a write's `expected_version` with the record's
version, a retired record's included: a record's count continues across
retirement, so a version read before the retirement is stale after it,
and the conflict names the retired record's version. A write expecting
that version re-creates the record and continues the count. Only a key
never written is at version 0.

## Why

An agent's decision is only auditable if one can reconstruct what it
saw. With a position on every reply, "what did the agent see" is a
store operation: read again `as_of` that position and get the same
answer. Without it, the question can only be approximated from
timestamps, and a timestamp does not order two writes in the same
millisecond.

A position also makes pagination and follow-up reads consistent. A
cursor bound to a position returns a stable page set across calls, and
a write's reply tells the caller exactly from which position its own
change is visible.

## What it is not

- A position is not a timestamp. Wall time is recorded on each event
  (as `known_at`), but only the position is exact.
- A position is not a record's version. The version is a per-record
  token for compare-and-set; the position is the lake-wide coordinate.
- Pinning `as_of` does not reopen history for writing. Writes always
  append at the head of the log.

## How it came to be

- The first version: a position on every answer, `as_of`, and the
  session pin.
- An earlier position is served from the history the projection keeps,
  not by a replay.
- Over MCP the session pin is the generated `pin` tool: it holds until
  cleared, a read's own `as_of` wins, and writes are never pinned
  (lakelands#9).
- A record's version continues across its retirement, so a version read
  before it never applies.
