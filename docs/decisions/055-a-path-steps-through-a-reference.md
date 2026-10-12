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

### A path that begins at a relation reads the one link the record holds under it

A path's first segment may name a relation of the verb's type carried
by link records instead of a field: `with.note` reads the field `note`
of the link the record holds under `with`, and from the link on the
path goes by the rule above, its `to` a reference to the far record
(`with.to.discount_days`). The link is a step toward `max_depth`, and so
is each reference after it. A record with no such link reads null
there, as a null reference does. A relation read so holds at most one
link from a record, `many_to_one` or `one_to_one`; one that may hold
several is refused at the declaration (`relation_holds_many`), as are a
derived relation (`relation_is_derived`), which holds no link record,
and one carried by a field (`relation_via_field`), whose edge the path
steps through by that field's name: one edge, one spelling. A
relation's name with no field after it stays an unknown field, and so
does a relation in a set's `where`, a trigger's `when` and a read's
`where`.
To an author who may not know of the link type, a path through it
names a field the verb's type does not have (`unknown_field`), whatever
the link holds, and nothing of the link type, as a read of the type
answers that author.

A verb whose path steps through a relation's link is its caller's only
where nothing of the link type and its end types is held from the
caller: it holds `restricted`, holds no `only` on the link type, on the
verb's own type or on the far end's type, is shown the relation, and
may read the link type and every field the path reads there. To any
other caller the verb is not its own: `not_granted` before any record
is read, and `describe` names it nowhere. A link the caller is not
answered therefore cannot arise at a call, and no answer of the call
tells of one. No link reads null; a link whose far end is retired goes
on by the rule below, `not_live` on `to`.

The softer forms were not taken. Reading a link the caller is not
answered as no link, as a read door answers it, would let a hold grant
the very call it was placed to stop: `not (with.blocked = true)` holds
for every caller a link is held from, and a guard decides a write that
is the lake's truth for everyone. Refusing such a call by a case of its
own tells its caller that a link it is not answered stands under the
record, a bit no other door gives every such caller: a link door that
would refuse a second link over a to-one relation is not open to a
caller who may not write the link type, nor to anyone for a relation
whose links come from a source.

The link and its ends count among what a proposed call reaches and
pins, and among what a migration or a declaration of the type that
carries the relation judges again, as a record read across does: a
type declared again with the relation holding several links, or with a
field under the relation's name, is refused naming the verb, as its
migration is.
A proposed declaration of such a verb carries no record: it is listed
by the rule of a proposed declaration
(`docs/decisions/013-verbs-run-by-declared-risk.md`), whole to a
reviewer shown the relation and every field the verb reads, whether the
verb would be that reviewer's to call or not.

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
- It is not a walk of a relation: a relation that may hold several
  links from a record is walked by a pattern; a path reads one link.
- It is not a set, a trigger or a derived field across an edge. A
  trigger's `when` judged only when its own record is written would
  miss every change of the far record, a level its author cannot rely
  on; such rules wait for a set that is kept across an edge.
- It is not a read's filter: a `where` that steps through a reference
  is refused as before.

## How it came to be

- A path may begin at a relation carried by link records and read the
  one link the record holds under it, where a relation's name was an
  unknown field in a condition; such a verb is its caller's only where
  the relation holds nothing from the caller (lakelands#567).
