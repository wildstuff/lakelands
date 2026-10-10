# 028 — Authoring is strict, and a bundle is refused whole

## What is decided

Every document Lakelands reads from an author — a bundle's YAML files
above all — refuses keys it does not know, naming them. Loading a bundle
is all or nothing: every problem is reported at once, each against its
file, and a bundle with any problem produces no declarations.

The typed form of an expression is strict in the same way, wherever it
arrives. A bundle's declarations must also hold together: a key, a
`depends_on`, a `via`, a `valid_from` or a view's `group_by` that names a
field the type (or the view's source) does not have is refused, and so
is every name that is not an ASCII identifier (a type name: identifiers
joined by `.`). A view's `where` and the aggregates in its measures are
held to the same rule, against its source and each merged source. A
name outside an aggregate is an earlier measure or a grouping field, as
`describe` reads a view, and is no source field. The migration
preflight reads a view by the same rule
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, extended).

## Extended: a bundle reaches a lake whole

A bundle's declarations are declared together through `Catalog::apply`:
validated as one set, so a declaration may name one the set declares
later, and appended in one append, so a lake holds all of the bundle or
none of it. The set is judged against the lake it would make: a type's
relations bring their link types with them (035), so a verb, a trigger,
an object set or a view of the same set may name one (lakelands#844),
and a field the set adds to one. A link type the lake holds is brought
as the set sends it, less what its author is not shown: a link type the
author is not shown is not brought and stays a type that is not
declared, and a field the author is not shown is left out and stays
unknown, in a set that declares its owner as in one that does not (006).

## Extended: a declaration is strict at every door

A declaration is strict wherever it arrives, as the bundle and the typed
expression already were:
- through `declare`, `apply`, `declare_type`, `add_field`, `migrate` and
  a proposal of a declaration or a migration, over MCP and REST;
- through the Rust contract itself, from JSON;
- through the log on replay, and through an export on import.

A member it does not take, at any depth, is refused as `invalid`. The
refusal names the member, the declaration, and where the member sits
(`fields[0]`, `relations[1].fields[0]`, `view.measures[2]`). Before this,
the JSON doors dropped such a member silently and answered as if it had
been stored, while the bundle refused the same member.

The rule lives on the contract's declaration types, not beside each
door, so a door added later cannot forget it. It changes no stored lake.
A reader already refused an event whose bytes did not re-serialise from
its typed form, so a log holding a member a declaration does not take
never opened. It now also says which member. No byte a lake writes
changes.

A client reads the declarations of its own version or older. A client
older than its server refuses a declaration member it does not know,
naming it, rather than dropping it, so a client is upgraded with its
server or before it.

## Why

A misspelt or retired key that is silently ignored leaves a setting at
its default with nothing saying why — the worst kind of error in a
declarative format, found long after it was written. Strictness costs
one line per document type.

All-or-nothing loading means a lake never holds half an ontology, and
reporting every problem at once lets an author fix a bundle in one pass
rather than one error at a time.

## What it is not

- It is not a freeze of the format. Adding a key is a change to the
  format and to its documentation; an old bundle keeps loading.
- It does not make the lake trust the bundle. Every declaration the
  loader produces is checked again when the lake declares it.

## Extended: a declaration reads only fields its type has (lakelands#461)

A view's `where` and measures were held to their sources' fields; the
expressions of the other declarations were only parsed. A derived field,
an object set's `where`, a trigger's `when` or a verb's `requires` that
named a field its type does not have was accepted, read that field as
null on every evaluation, and so never held, or always did, with nothing
said. Now `declare`, `apply` and an accepted proposal of a declaration
refuse it as invalid, naming the declaration and the field:
- a type: what each derived field's expression reads;
- an object set: its `where`;
- a trigger: its `when`, where `changed(f)` and `previous(f)` read `f`;
- a verb: its `requires`, and what the value it sets a field from reads.
  `payload.<name>` there is the call's payload and no field.

`setting`, `now`, `today` and a guard's `retired` read no field. The
reads are gathered by the function the migration preflight keeps a
field by (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended), so what `declare` takes, a migration keeps. A migrated type
is checked as a declared one is: a migration to a type whose derived
field reads a field it does not have is refused the same way, by the
preflight, once the fields the migration itself takes away have been
named. A
declaration already in force is not checked again until it is declared
again.

## Extended: the fields a declaration reads are those its author is shown (lakelands#538)

"A field its type has" is said for the one who declares. A declaration
is checked as sent against the ontology as its author is shown it,
before it is checked against the lake's own, so a field its author may
not know of is refused as a field the type does not have, by the same
check and in the same words. What a declaration reads is gathered as
before, with two additions: what a view reads of a source, and the own
side of a derived relation's pairs. The fields a verb sets are kept
apart from what it reads: its author must know of them, not read them.
The rule and its reasons are in
`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended.

## Extended: in a verb's value only `payload.<name>` is the call's (lakelands#470)

The rule above left the name `payload` out of what a verb's value
reads, wherever it stood. But the call's payload is reached by exactly
one form, `payload.<name>`: a bare `payload`, a longer path such as
`payload.note.deep`, and either of them inside a function are the
record's field of that name. On a type without such a field they were
accepted and read null at every call, so the verb cleared the field it
sets; and a migration could take a field named `payload` away while a
verb read it bare, with the same end.

One function now says which path is the call's, and the verb's scope
that evaluates a value and the rule that gathers what a declaration
reads both ask it:

- **`payload.<name>`** is the call's payload and reads no field.
- **Every other path** is the record's and reads its first segment,
  `payload` included. `declare`, `apply` and an accepted proposal refuse
  it on a type that has no such field, as for any other missing field,
  and the migration preflight keeps the field while a verb reads it.
- **A member the verb's payload does not declare** is refused too,
  naming the verb, the field it sets and the member. A call's payload is
  checked against the verb's list, so such a member is never given and
  the value would read null at every call.

A type may have a field named `payload`, and a verb on it is not
refused: the two readings sit one segment apart, and the rule is total.
What follows is that a verb's value cannot read a member of a json field
named `payload`, since `payload.<name>` is always the call's. A lake
that needs it names the field otherwise, or reads it whole.

A verb already in force with a bare `payload` opens and runs as before;
it is held to the rule when it is declared again.

## Extended: the order of refusals (lakelands#338)

The schema doors refuse in this order, in the frame every write door
shares (016, extended): the lake, the context, the door's own checks,
the append.

**`declare`**: the `schema` grant (`not_granted`); the declaration's
own checks, strict and whole as above; then what it needs bound: a
trigger's `max_depth` within the lake's (`invalid`), a backed
function's backing (`unbound`), each embedding's model (`unbound`).

**`apply`**: the `schema` grant; a set that is not empty (`invalid`);
then every declaration's own checks and bound checks, collected: one
problem answers as itself, several answer one `invalid` naming them
all.

**`add_field`**: the `schema` grant; the type is declared
(`not_found`); it has no field of that name (`invalid`, naming it);
then `declare`'s checks on the type with the field added.

A trigger, a function and a type each need one thing bound, so the
bound checks never meet in one declaration, and an undeclared type has
no field: those pairs are stated, not checked.

## Extended: what can never be evaluated is refused (lakelands#549)

An expression that fails is, to the fold and to a read, a record that
does not match, a null key or a derived field that is cleared. That is
right where the failure is one record's: a division by a field that is
zero in one record must not fail a read of the others. It was wrong
where the failure is the expression's own. A derived `number` field that
is `today() - due_on` was declared and never filled, and a `where`
multiplying a boolean answered no record, each with nothing said.

What an expression computes is now read off it where that is certain: a
literal's kind, a function's result, arithmetic over certain kinds, a
field by its declared kind (`lakelands_api::expr::kind`). Where it
depends on the record (a `json` field, `if` or `coalesce` over two
kinds, a setting, a parameter) nothing is said, and a text is never
certain to fail, since one that reads as a number or an instant is one.
Two rules hold by it:

- **A derived field its expression can never fill is refused,** as
  `invalid` on the field, by `declare`, `apply`, `add_field`, a migration
  and a proposal. The fold lands a value by the field's kind, and what it
  never lands is: a duration, a boolean or an instant in a `number` or
  `decimal` field; anything but a boolean in a `boolean` field; a
  duration, a boolean or a number in a `date_time` field. A duration says
  how it becomes a number: divided by a unit.
- **An operation that has no meaning is refused,** as `invalid`, naming
  it: an instant plus an instant, a boolean in arithmetic, a duration
  plus a number, a function of a number given an instant. In a derived
  field, a trigger's `when`, an object set's `where` and a verb's
  `requires` and values at the declaration; in a read's `where` and
  group keys at the door, once the reader is known to read the fields
  named, so the refusal tells nothing of a field it is kept from.

A failure that depends on a record's value stays that record's. A
declaration in force is not checked again until it is declared again.

## Extended: what a view computes lands as a derived field does (lakelands#563)

A view's measure `max(due_on)` declared into a `number` field was taken,
and a read answered the view's records without it: the value never
lands, and nothing said so. Each measure of a view and of its merged
sources, read over its source's fields, and each group value are held to
the rule above for a derived field, at the declaration: one that
certainly computes a kind its field never holds is refused as `invalid`
on that field, naming the kind. A name a measure answers under is read
as nothing certain, since an expression may mean the measure by it.

## Extended: a path steps only into a value that has members (lakelands#552)

A path's first name was checked against the type, and the rest was not:
`partner.name = 'x'` on a reference read null for every record, so a set
declared with it had no member, a trigger never fired and a read
answered nothing, each with nothing said.

A path of more than one name is a walk into a `json` or `geo` value's
members. Into a field of any other kind it is refused as `invalid`,
where lakelands#549's rule refuses what can never be evaluated: at the
declaration of a set, a trigger, a verb and a derived field, and at a
read, once the reader is known to read the field. The refusal of a step
through a reference says what to write: a condition reads the record's
own fields; compare the reference's key, or walk to the record it names
in a pattern. Reading through a reference in a condition is not built
(lakelands#567). A declaration in force that holds such a path stays in
force until it is declared again.

## Extended: a valid time is a date-time field (lakelands#714)

`valid_from` and `valid_to` had to name a field of the type, of any
kind. A read at `valid_at` anchored a record by any string value that
reads as an instant, so a `text` or a `decimal` field, or a reference
whose key looks like a date, set a record's validity by coincidence. A
value that did not read as one left the record valid at no instant, with
nothing said, and a migration could turn the valid time into `text`.

Each now names a field of kind `date_time`, stored or derived, on a type
or a view. A field of any other kind is refused as `invalid` on it,
naming the member and the kind, by `declare`, `apply`, a proposal and a
migration's `to`. A write holds a
`date_time` value to a UTC instant, so a valid time is an instant or
absent, and the read anchors only those. A declaration in force is not
checked again until it is declared again, and then it is checked as any
other is.

## Extended: a date is a valid time too (lakelands#702)

A valid time names a field of kind `date_time` or `date`. A date is the
instant at its UTC midnight, so a booking valid from `2026-07-01` to
`2026-07-31` holds from the start of the 1st up to the start of the
31st, not through it: the interval stays half-open, as for instants. A
field of any other kind is refused as before, and the refusal names both
kinds a valid time may be.
