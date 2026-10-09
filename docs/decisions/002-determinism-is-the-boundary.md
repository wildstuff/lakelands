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

## Extended: a seam's failure is its output

What a seam answered is recorded, whether it answered a value or
failed. A failure is recorded as `{"error": "<why>"}` and read back on
replay like any other output, so a replay reproduces the failure
exactly, and the store never calls the seam again for the same input to
see whether it would answer differently now. The store calls a seam
only after the append that made the call due, outside the fold: the
fold reads the log and the outputs recorded in it, and nothing else
(`docs/decisions/022-functions-are-declarations.md`, extended).

## Corrected (2026-09-25): a redeclaration calls again

"The store never calls the seam again for the same input" holds for a
replay and for every later event that touches the record. It has one
deliberate exception: a `declared` event of the function or the type
retries a recorded failure once, and the later output holds. Since
lakelands#565 the door `recompute` appends that event; a declaration in
force appends none (`docs/decisions/022-functions-are-declarations.md`,
"Extended: `recompute` computes again, and retries a recorded failure").

## Extended: an output the log holds is named, not carried again

A replay still reads every output from the log, never from the seam:
where an output the lake holds fills a field again, the event names the
output by its position, and the fold reads it there and checks that it
is the call's latest
(`docs/decisions/053-a-recorded-output-is-recorded-once.md`).
