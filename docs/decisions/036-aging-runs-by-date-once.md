# 036 — Aging runs by date, once

## What is decided

An **aging rule** moves a field of every record of a type from one
value to another when a condition holds on a date: an open invoice
becomes overdue thirty days after it fell due. A rule is a record of the
system type `sys.aging_rule` (`on`, `field`, `from`, `to`, `when`),
written through the ordinary doors under grants and checked when it is
written; `docs/seams.md` gives the shape.

`LakeStore::age(today)` runs every rule for one date. Inside a rule's
condition, `today()` and `now()` are that date at midnight, never the
clock of the process running it. A record moves when its field holds
the rule's `from` and the condition holds; the move is an ordinary
`record_upserted`, written by the store with the rule as its procedure,
one append per rule. Every move has an id made from the rule, the record
and the date (`docs/log-format.md` §3), and the store appends nothing
for an id already in the lake: aging a date twice writes it once. A
condition that cannot be evaluated for a record leaves the record as it
is.

## Why

Time passing is not an event the lake can see; a date must be handed
to it. Making the date an argument, rather than the clock, is what keeps
aging inside the determinism boundary
(`docs/decisions/002-determinism-is-the-boundary.md`): the same rules on
the same lake for the same date make the same moves, and the moves are
in the log, with their rule and its version as provenance, so a replay
reads them instead of running them again.

Idempotency by date is what lets a ticker, a host's scheduler or an
operator's command run aging without coordinating: running a day twice,
or from two schedulers, is harmless. Moving only records that hold
`from` makes a rule a transition, not an overwrite, so a value a person
set since the last run is not trampled. A failed condition leaves a
record alone because aging is a background process: no caller is there
to see an error, and doing nothing is the safe default.

Rules are records rather than declarations because they are tuned as
data: a lake's operator changes a threshold without changing the lake's
shape, and every change is a version with its writer.

## What it is not

- It is not a scheduler. The store ages the date it is given; when to
  call it is the ticker's or the host's business.
- It is not a trigger. A trigger detects that a write made something
  true; aging makes a write because a date made something true, and its
  writes fire triggers like any other.
- It does not reach outside the lake. A rule only moves a value; what
  should happen because an invoice is overdue is a trigger's and a
  host's.
