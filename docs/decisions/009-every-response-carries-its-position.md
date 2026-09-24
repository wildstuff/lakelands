# 009 — Every response carries the position it was read at

## What is decided

Every answer the store gives, to a read or a write, carries the
`position`: the log coordinate the answer reflects. A read can be
pinned to an earlier position with `as_of`, and a session (an MCP
session, for instance) can pin one `as_of` for a whole turn so that
every read in it sees the same state.

Business time is a separate axis. `valid_at` asks what was true in the
world at a moment; `as_of` asks what the lake knew at a position. The
two combine, and neither stands in for the other.

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
