# 055 — A path steps through a reference

## What is decided

### A verb's condition and values read a field of the record a reference names

In a verb's `requires` and in the values it sets, a path steps through a
reference field to the one record it names and reads a field there:
`customer.discount_days`, `customer.term.days`. A link's `from` and `to`
are references of the link record, so a verb on a link type reads its
ends by the same rule (`from.grade`, `to.discount_days`). A path takes
as many steps as the lake's `max_depth`; one that takes more is refused
`step_too_deep` at the call, not at the declaration: the limit is the
host's and may differ at the next open, and a declaration valid in the
ontology stays valid. A reference that is null reads null, as a field
that holds no value does.

A path is read at the call, under the writer, at the head and over the
append's own drafts, where `requires` is judged: each record it reaches
once a call, and only where the evaluation comes to the path, so a
condition decided before it reads no record. What the call wrote is the
log's: it stays when the far record changes after the call, and a
rebuild gives it again.

A path that cannot be read is refused at the declaration: a far field
the type does not have (`unknown_field`, naming the far type), one
computed at the read (`reads_at_read`), a step into a far field that is
no reference and has no members (`steps_into_scalar`), and `changed` of
a path (`call_takes_field`). An object set's `where`, a trigger's
`when`, a derived field and a read's `where` keep the refusal of a step
through a reference
(`docs/decisions/028-authoring-is-strict-and-whole.md`).

### At a write a far record is reached exactly where a write that sends the reference would be accepted

A record read across is reached where an upsert that sends its key in
that reference would be accepted
(`docs/decisions/032-a-reference-names-a-live-record.md`), and by
nothing else: one held from the caller, outside its `only`, retired or
purged refuses the call `not_live`, naming the reference and the key the
record holds, the answer the caller's own upsert sending that key gets,
whole; a second step to such a record names the far reference. It is
never read as null at a write, whatever the record's fields hold. The
refusals come in one order: the record is found (`not_found`), its
version compared (`conflict`), then what `requires` reaches
(`not_live`), then `requires` itself (`precondition`).

032 has no valid time in it: a far record outside its valid time is
live and is read at a write. A read that walks to a record keeps the
walk's rule, the record valid at the read's `valid_at`
(`docs/decisions/026-the-store-walks-the-graph.md`).

### A verb reads what its paths read, wherever a principal is judged by what it reads

The fields a verb reads are its record's and every field its paths
read, with each reference on the way and the types they name. That one
list is what decides whether the verb is its caller's and is described
to it, what a proposed call reaches and pins, and what a migration of a
type a verb reads across keeps
(`docs/decisions/006-grants-not-roles.md`). A migration that takes a
field a verb reads across, or takes it to the read, is refused naming
the verb; and every verb on a migrated type or on a link type its
relations carry, or reading across into one of them, is judged again
over the lake as the migration leaves it, so a reference retargeted
under a path, on the verb's type or a far one, a relation retargeted
under a verb on its link type, or a kind changed under a verb's own
condition, is refused before it applies.

What a proposed call reads across is counted as it read when it was
proposed and as it reads now: a condition a setting or the clock decides
may come to a record at one and not at the other, and the proposer was
judged on the first.

## Why

A cash discount is granted while an invoice is paid within its
customer's period: the period is the customer's, and a verb on the
invoice must read it where the call is decided, at the head. Copying it
onto every invoice is a field that goes stale; reading it across the one
reference the invoice already holds is the edge the relation walks.

A far record not reached is not null at a write. Were it null, a call
whose condition holds over null would run against a customer its caller
is not answered, and a call that copies a far field would copy what its
caller may not read: one call a bit, the oracle that a probe's rule
closes (006). Refusing it as the caller's own upsert of that key is
refused tells the caller nothing it could not learn by writing the key.

## What it is not

- It is not a join. A path reads one record a step, the one a reference
  names; walking to-many relations is the pattern's job
  (`docs/decisions/018-one-pattern-query.md`).
- It is not a relation's name: a relation is walked by a pattern, and
  the value on the link between two records is not read by a path here.
- It is not a set, a trigger or a derived field across an edge. A
  trigger's `when` judged only when its own record is written would
  miss every change of the far record, a level its author cannot rely
  on; such rules wait for a set that is kept across an edge.
- It is not a read's filter: a `where` that steps through a reference
  is refused as before.
