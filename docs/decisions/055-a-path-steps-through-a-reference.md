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
a path (`call_takes_field`). An object set's `where` and a trigger's `when`
read across an edge by the section below; a derived field and a read's
`where` keep the refusal of a step through a reference
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
several is refused at the declaration (`relation_holds_many`), as is
one carried by a field (`relation_via_field`), whose edge the path
steps through by that field's name: one edge, one spelling. A
relation's name with no field after it stays an unknown field, and so
does a relation in a read's `where`.
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

### A path may begin at a derived relation onto its target's key, and `exists` asks whether a path reaches a record

A path's first segment may name a derived relation of the verb's type
(`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`)
whose match names every key field of its target: `discountable.rate`
reads the field `rate` of the one record it matches, and goes on from
there by the rules above. It reaches one record at most by
construction, the record at the key its match names, whatever its
declared cardinality says, since 044 enforces none. A derived relation
whose match is not its target's key, or whose target keys nothing or
is a view, is refused at the declaration (`match_not_key`), naming the
relation and its target: it may reach several records, and a verb's
condition has one record in scope.

At a call the record is read by its key, under the writer at the head
and over the call's drafts, as a reference's is; its matched values are
then compared as 044 compares them, and the relation's `where` is
evaluated over the two records with the call's clock and settings. A
record that is not there, is retired, does not hold the matched values
as stored or for which the `where` does not hold is no match: the path
reads null. It is never `not_live`: no reference names the record, and
044 requires no live target. The match is a step toward `max_depth`. A
record at that key that is there and not answered to the judged
principal, which a hold placed since a proposal can make so at its
accept, refuses the call `not_granted`, as a link does: read as no
match, it would let a hold grant `not exists(…)`.

`exists(<relation>)` answers whether the path that begins at the
relation reaches a record: for a derived relation onto the key, the one
record it matches; for a relation carried by links, the one link the
record holds under it. It stands in a verb's `requires` and set values
only, over a relation a path may begin at, and reads what such a path
reads: of a link, its `from`; anywhere else, and over a field, it is
refused (`exists_here`), and over another relation as a path from it is.
No `exists` over several records and no count: a count is an aggregate,
which needs a set of records in scope
(`docs/decisions/017-one-expression-grammar.md`), and a scan at every
call under the lake's one writer would cost what data no author
controls; how many records an edge reaches is the business of a set.

A verb that reads a derived relation is its caller's only where nothing
of the target and of the verb's own type is held from the caller: it
holds `restricted`, no `only` on either, is shown the relation, and may
read every field the match and the `where` read on both sides and every
field the path reads there. To any other caller the verb is not its own,
`not_granted` before any record is read, as for a link. A target that is
a link type holds its ends too: an `only` on an end's type holds the
verb as an `only` on the target does. A derived edge does not differ
from a link here: a held target read as no match would let `not
exists(discountable)` pass for exactly the caller the terms are held
from, and refusing at the call would tell that a held record matches.

The record matched counts among what a proposed call reaches, as a
record read across does, and the verb reaches by state: the match moves
with the data and the clock. A migration or a declaration that takes
the match off the target's key, changes the target's key, or takes a
field the match or the `where` reads, is refused naming the verb.

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

### A set's `where` and a trigger's `when` read across an edge, decided again at every write of the far record

A set's `where` and a trigger's `when` take every path a verb's
condition takes, and are refused at the declaration by the same cases:
through a reference to `max_depth`, from a relation carried by links to
its one link, from a derived relation onto its target's key, and
`exists(<relation>)`. `changed` and `previous` of a path stay
`call_takes_field`. A set is decided for no reader
(`docs/decisions/004-the-change-feed-is-the-log.md`), so a far record is
read at the head, whoever is answered it, and over the append's own
drafts: one that is not live (retired, purged, never there) reads null,
and a hold changes nothing. A path past `max_depth` reads null too.
A path into a far field computed outside the lake (a backing's answer,
an embedding), or derived from one at any depth, is refused at the
declaration, `set_reads_computed` naming it: that value lands in an
append of its own that writes no record, so no write would decide the
referrers again, and a fill never decides one.

**What decides a membership again.** Every write of a far record, in its
own append and caused by its own event, whatever door writes it:
`upsert`, a verb, an accept, `ingest`, `retire` and revival, a purge, an
aging move. It pays only where a field some set's path reads on that
record moved, or the record became live or not live; any other write
pays nothing. The records decided are those that reach it, found by
stepping back along the path a type at a time: by the reference's key in
one scan of the referring type (the exact-text index of 057 where an
engine keeps one), by a link's `from`, by a derived relation's matched
values. The step back goes by the path's steps, never by type, so a
reference to a record of the same type (a manager, a parent) is stepped
back through like any other. Each is decided by the one rule a write
and a declaration decide by, its memberships read once a set and a
write. A trigger whose
`when` reads across fires once for each record that reaches the far
record and whose `when` holds after that write: the level its author
states, judged at the write that moved what it reads.

A migration, a re-key, a value type's rename and a `declared` that
computes a far type's fields again decide the referring sets in their
own preflight, the far records read as the step leaves them; a
migration that renames or takes a field such a set or trigger reads is
refused naming it, as for a verb.

**The bound.** Past the lake's `max_set_changes`, counted over every
membership change and every firing far writes set off in one append,
the write is refused `too_large`, naming the set or the trigger and the
ceiling to a caller shown it, and naming nothing to another; nothing is
appended. A purge is refused whole, and its preflight, a read that takes
no lease, reports how many it would set off beside the ceiling; an
aging move past it is counted as refused (lakelands#1182). The write lands once fewer
memberships move: narrow the set by the referring record's own fields,
since only those that move count; or retire the set and declare it again
after the write, its declaration bounded alike; or have the lake opened
with a higher `max_set_changes`. A write itself cannot be split: one
field of one record moves every referrer at once.

**Whose it is.** A membership tells a far field: `in_set('graded')` on an
invoice tells `customer.grade`. So a set or a trigger whose condition
reads across an edge is a reader's only where nothing of a far type is
held from it: it holds `restricted`, no `only` on a far type nor, of a
link type, on its ends, and reads every reference and far field on the
way. To any other reader the set is one that is not declared
(`set_undeclared` in a read's `in_set`), `describe` names neither, and
their events are not told to its subscription. A writer is told that
such a set moved, or such a trigger fired, at records other than its
write's own only where it is answered one of them, as a write's own
record is its writer's. Then no two lakes that differ only in a record
held from a reader answer that reader differently.

**A read's `in_set` of such a set reads its kept membership.** In a
read's `where`, a set across an edge stays `in_set('name')`, not its
filter (a path is no read's filter), and is answered by the members the
writes kept: at the read's position, as of an earlier one too, the
members read once a read through the seam's `members`. It is the one
place a set's members are read beside the feed. The store evaluates such
a condition itself, as it does one that reads the clock: no engine is
handed it, and no engine changes. A set that names such a set is read the
same way through it. To a reader it is not shown it stays
`set_undeclared`.

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
  links from a record, or a derived relation off its target's key, is
  walked by a pattern; a path reads one link or one matched record.
- It is not a derived field across an edge. A derived field is computed
  at its record's write and kept; one that read a far record would go
  stale at every far write, and computing it again there is a write of
  the referrer no one made.
- It is not a read's filter: a `where` that steps through a reference
  is refused as before.

## How it came to be

- A path may begin at a relation carried by link records and read the
  one link the record holds under it, where a relation's name was an
  unknown field in a condition; such a verb is its caller's only where
  the relation holds nothing from the caller (lakelands#567).
- A path may begin at a derived relation onto its target's key, and
  `exists(<relation>)` asks whether a path reaches a record, of a
  derived relation and of one carried by links; a derived relation,
  refused in a path before (`relation_is_derived`), is now refused only
  off its target's key (`match_not_key`) (lakelands#960).
- A set's `where` and a trigger's `when` read across an edge, decided
  again at every write of the far record that moves what they read
  there, under one bound, and a reader's only where nothing far is held
  from it; before, both were refused a step through a reference, since
  a condition judged only at its own record's write would miss every
  change of the far record (lakelands#567).
