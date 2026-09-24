# 002 — Determinism is the boundary

## What is decided

What belongs inside the store and what belongs outside it is decided by
one test: must the result reproduce byte-identically when the log is
replayed?

If yes, it lives in the store and runs there. Derived fields, functions
evaluated by the built-in expression backing, value-type constraints,
views, trigger detection, the fold's merge rules and the materialised
per-field metadata are all computed from the log alone, and a replay
recomputes them exactly.

If it needs the outside world (a network, a model, a sandbox, a process,
a clock other than the event's own time), it sits behind a seam. The
store calls the seam, records its output as an event with provenance,
and on replay reads that recorded output instead of calling the seam
again.

## Why

Replay is how Lakelands answers "what did the lake say at position N"
and how a projection is rebuilt (see 001). Both break the moment a
replay can produce a different answer than the original run. Drawing the
boundary at determinism keeps replay honest without having to trust
every integration to be stable.

It also gives the store a small, testable core. Everything inside the
boundary is a pure function of the log and can be checked by the
conformance suite on every engine; everything outside is a seam with a
narrow contract that a host implements.

## What it is not

- It is not a rule that the store avoids useful work. Detection happens
  inside: the store decides that a trigger fired or a verb was called,
  and records that as an event. Only the execution that needs the
  outside world is behind a seam.
- It is not a licence for seams to hold invariants. Field rights,
  provenance stamping, the `sys.` guard and idempotency are enforced by
  the store on every write, never by a seam implementation that a direct
  caller could bypass.
- Nothing in the store calls a model, starts a sandbox or opens a
  network connection on its own.
