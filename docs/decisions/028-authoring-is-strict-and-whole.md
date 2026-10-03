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
none of it.

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

