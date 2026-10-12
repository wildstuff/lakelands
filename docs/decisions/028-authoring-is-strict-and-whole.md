# 028 — Authoring is strict, and a bundle is refused whole

## What is decided

### A document refuses a key it does not know, and a bundle loads whole

Every document Lakelands reads from an author — a bundle's YAML files
above all — refuses keys it does not know, naming them. Loading a bundle
is all or nothing: every problem is reported at once, each against its
file, and a bundle with any problem produces no declarations.

The typed form of an expression is strict in the same way, wherever it
arrives.

### A bundle's declarations hold together

A bundle's declarations must hold together: a key, a `depends_on`, a
`via`, a `valid_from` or a view's `group_by` that names a field the type
(or the view's source) does not have is refused, and so is every name
that is not an ASCII identifier (a type name: identifiers joined by
`.`). A view's `where` and the aggregates in its measures are held to
the same rule, against its source and each merged source. A name outside
an aggregate is an earlier measure or a grouping field, as `describe`
reads a view, and is no source field. The migration preflight reads a
view by the same rule
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, extended).

### A bundle reaches a lake whole

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

### A declaration is strict at every door

A declaration is strict wherever it arrives, as the bundle and the typed
expression are:
- through `declare`, `apply`, `declare_type`, `add_field`, `migrate` and
  a proposal of a declaration or a migration, over MCP and REST;
- through the Rust contract itself, from JSON;
- through the log on replay, and through an export on import.

A member it does not take, at any depth, is refused as `invalid`. The
refusal names the member, the declaration, and where the member sits
(`fields[0]`, `relations[1].fields[0]`, `view.measures[2]`).

The rule lives on the contract's declaration types, not beside each
door, so a door added later cannot forget it. A reader refuses an event
whose bytes do not re-serialise from its typed form, so a log holding a
member a declaration does not take never opens, and the refusal says
which member.

A client reads the declarations of its own version or older. A client
older than its server refuses a declaration member it does not know,
naming it, rather than dropping it, so a client is upgraded with its
server or before it.

### A declaration reads only fields its type has

A derived field, an object set's `where`, a trigger's `when` or a verb's
`requires` that names a field its type does not have would read that
field as null on every evaluation, and so never hold, or always do, with
nothing said. `declare`, `apply` and an accepted proposal of a
declaration refuse it as invalid, naming the declaration and the field:
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
named. A declaration already in force is not checked again until it is
declared again.

### The fields a declaration reads are those its author is shown

"A field its type has" is said for the one who declares. A declaration
is checked as sent against the ontology as its author is shown it,
before it is checked against the lake's own, so a field its author may
not know of is refused as a field the type does not have, by the same
check and in the same words. What a declaration reads is gathered as it
is for its type's fields, with two additions: what a view reads of a
source, and the own side of a derived relation's pairs. The fields a
verb sets are kept apart from what it reads: its author must know of
them, not read them. The rule and its reasons are in
`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
extended.

### In a verb's value only `payload.<name>` is the call's

The call's payload is reached by exactly one form, `payload.<name>`: a
bare `payload`, a longer path such as `payload.note.deep`, and either of
them inside a function are the record's field of that name. Accepted on
a type without such a field they would read null at every call, so the
verb would clear the field it sets; and a migration could take a field
named `payload` away while a verb read it bare, with the same end.

One function says which path is the call's, and the verb's scope that
evaluates a value and the rule that gathers what a declaration reads
both ask it:

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

### What can never be evaluated is refused

An expression that fails is, to the fold and to a read, a record that
does not match, a null key or a derived field that is cleared. That is
right where the failure is one record's: a division by a field that is
zero in one record must not fail a read of the others. It is wrong
where the failure is the expression's own: a derived `number` field that
is `today() - due_on` would be declared and never filled, and a `where`
multiplying a boolean would answer no record, each with nothing said.

What an expression computes is read off it where that is certain: a
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

### Provenance is read only where a record stands

`seen_at(f)` and `known_at(f)` read the provenance of a record's field
(`docs/decisions/010-provenance-per-field.md`). Three places evaluate
an expression where no record stands: a value type's `constraint`, over
a value alone; a function's `expr`, over the inputs it is handed; and a
view's `where` and measures over a source that is itself a view, whose
records are computed and carry none. There the two functions would read
nothing and say nothing of it, so a declaration that calls them there
is refused as `invalid` (`no_records_provenance`), at its member, by
`declare`, `apply` and `propose` alike. A view over a stored type reads
its source records' provenance and is not refused; a stored type cannot
become a view (`migration_to_view`), so what a view's source is is
settled where the view is declared.

A read asks the same of itself: over a view's records the `where` of a
`find` and of a pattern's start or step, a `find`'s group key and its
order by `seen_at` or `known_at` are refused alike when they call for
one of the two, where a read over a stored type is answered. A
pattern's rows are no records whatever its types are: an order of them
by `seen_at` or `known_at` is refused over a stored type too. A search
over a view is refused whatever its filter reads (`view_not_searched`):
a view is not searched. A read that answers nothing and says nothing of
why is the defect on the other side. A refusal is lifted, with its row,
when one of these is given a provenance to read.

### What a view computes lands as a derived field does

A view's measure `max(due_on)` declared into a `number` field would be
taken, and a read would answer the view's records without it: the value
never lands, and nothing would say so. Each measure of a view and of its
merged sources, read over its source's fields, and each group value are
held to the rule for a derived field its expression can never fill, at
the declaration: one that certainly computes a kind its field never
holds is refused as `invalid` on that field, naming the kind. A name a
measure answers under is read as nothing certain, since an expression
may mean the measure by it.

### A path steps only into a value that has members

Were a path's first name checked against the type and the rest not,
`partner.name = 'x'` on a reference would read null for every record, so
a set declared with it would have no member, a trigger would never fire
and a read would answer nothing, each with nothing said.

A path of more than one name is a walk into a `json` or `geo` value's
members. Into a field of any other kind it is refused as `invalid`,
where what can never be evaluated is refused: at the declaration of a
set, a trigger, a verb and a derived field, and at a read, once the
reader is known to read the field. The refusal of a step through a
reference says what to write: a condition reads the record's own fields;
compare the reference's key, or walk to the record it names in a
pattern. Reading through a reference in a condition is not built
(lakelands#567). A declaration in force that holds such a path stays in
force until it is declared again.

### A valid time is a `date_time` or a `date` field

`valid_from` and `valid_to` each name a field of kind `date_time` or
`date`, stored or derived, on a type or a view. A field of any other
kind is refused as `invalid` on it, naming the member and the kind, and
both kinds a valid time may be, by `declare`, `apply`, a proposal and a
migration's `to`.

Were a valid time a field of the type, of any kind, a read at `valid_at`
would anchor a record by any string value that reads as an instant, so a
`text` or a `decimal` field, or a reference whose key looks like a date,
would set a record's validity by coincidence. A value that does not read
as one would leave the record valid at no instant, with nothing said,
and a migration could turn the valid time into `text`.

A write holds a `date_time` value to a UTC instant, so a valid time is
an instant or absent, and the read anchors only those. A date is the
instant at its UTC midnight, so a booking valid from `2026-07-01` to
`2026-07-31` holds from the start of the 1st up to the start of the
31st, not through it: the interval stays half-open, as for instants. A
declaration in force is not checked again until it is declared again,
and then it is checked as any other is.

### The schema doors refuse in one order

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

## How it came to be

- The first version: a document refuses a key it does not know, a
  bundle loads whole or not at all, and its declarations name only
  fields their types have.
- A bundle is declared through `Catalog::apply`, as one set in one
  append.
- A declaration became strict at every door. Before, the JSON doors
  dropped a member they did not take silently and answered as if it had
  been stored, while the bundle refused the same member; a reader
  already refused such an event and came to say which member. The change
  altered no stored lake and no byte a lake writes (lakelands#240).
- A view's `where` and the aggregates in its measures joined its
  `group_by` in being held to its sources' fields (lakelands#391).
- The order in which the schema doors refuse was stated
  (lakelands#338).
- The expressions of a type, an object set, a trigger and a verb, only
  parsed before, are held to the fields their type has (lakelands#461);
  a migrated type is then checked by the preflight, once the fields the
  migration takes away have been named.
- The name `payload`, left out of what a verb's value reads wherever it
  stood, is the call's only as `payload.<name>` (lakelands#470).
- A declaration is checked against the ontology as its author is shown
  it, and what it reads gained a view's reads of a source and a derived
  relation's own side (lakelands#538).
- An expression that can never be evaluated, and a derived field its
  expression can never fill, are refused; before, each failed at every
  record with nothing said (lakelands#549).
- A path of more than one name, of which only the first was checked, is
  refused outside a `json` or `geo` value (lakelands#552).
- A view's measures and group values are held to the kind their fields
  hold, as a derived field is (lakelands#563).
- `valid_from` and `valid_to`, which could name a field of any kind,
  name a `date_time` field (lakelands#714).
- A `date` field is a valid time too, the instant at its UTC midnight
  (lakelands#702).
- A set is judged against the lake it would make, with the link types
  its relations bring; before, a type was put in without them, so a
  set that declared a relation and a verb or a trigger on its link
  type was refused as not declared on a lake that did not hold the
  relation yet (lakelands#844).
- `seen_at` and `known_at` are refused where no record stands: declared
  in a value type's constraint, a function's expression or a view over
  a view, and read in a `where` over a view's records, each of which
  read nothing before and said nothing of it (lakelands#852).
- A `find`'s group key and its order read no provenance over a view's
  records either: the key answered one group `false` and the order
  answered as if none were asked. The sentence on a search's filter
  said "refused alike"; a search over a view is refused before its
  filter is read (lakelands#889).
- A pattern's rows ordered by `seen_at` or `known_at` were answered in
  the order of the value, over a view's records and a stored type's
  alike. They are refused, as an order of them by `latest` is
  (lakelands#901).
