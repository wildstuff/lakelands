# 035 — A link is a record

## What is decided

A relation declared without `via` is carried by **link records**: one
record per linked pair, of a type the store derives from the relation,
`sys.link.<type>.<relation>`, keyed by the pair (`from`, `to`) and
holding the relation's own fields. `docs/log-format.md` §2.3 specifies
it. Links are written and removed by the record events the log already
has (`record_upserted`, `record_ingested`, `record_retired`); no event
kind is added, and the log format does not change.

The catalog gains a pair of doors per relation carried by links:

- `link(who, type, key, relation, to, fields, opts)` writes the link from
  the record `key` of `type` to the target `to`, with the link's fields
  as a sparse patch; writing it again changes its fields.
- `unlink(who, type, key, relation, to, opts)` removes it.

Both answer an `Outcome` and check the relation's `authority` (who
writes its links: `authored` by default, `source` for links that
arrive by ingest) and the principal's grants on the link type's fields,
as `upsert` does. The tool generator makes one tool per door and
relation, named after the type and the relation, so that two types'
relations of one name cannot collide. The WIT package and its drift
check follow the new doors.

The pattern query walks a link relation as it walks a `via` one; an
engine answers its `edges` from the live link records at the view's
position, with the link's fields in `Edge.fields`. A relation's fields
are shape (`docs/decisions/007-schema-hash-covers-shape.md`), so a change
to them is declared or migrated by
`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, on the link
type as on any type.

## Why

A link with fields is a fact with a history: when it was made, by whom,
from which source, what its fields said then. Records already carry all
of that: versions, per-field provenance, `as_of`, retirement that keeps
history, rebuild, verification, the change feed. Two new event kinds
would have to grow every one of those again, and every reader of the
log would need a format version 2 to read them. Carrying links as
records gives them everything records have, and changes nothing a
version 1 reader knows.

Keying a link by its pair makes a relation a set: a second write of the
same link changes its fields, never adds a duplicate, and cardinality is
a check over live links rather than a convention.

Naming the link type under `sys.` keeps the writes where the rules are:
the `sys.` guard stops a caller from declaring or reshaping it, and the
ordinary doors refuse its records, so a link is made only through the
doors that check the relation's authority and cardinality. A verb
declared on the link type (003, extended) writes a live link's own
fields and nothing else: under the same authority, with both ends
checked as `link` checks them, live and answered to its caller, and
never an end. A `set` that names `from` or `to` is refused when the
verb is declared, as `link` refuses an end among its fields.

## What it is not

- It is not a second way to link. A relation is carried either by its
  `via` field or by link records, never both, and a declaration that
  would move a relation from one to the other is refused.
- It is not a graph store beside the lake. Link records live in the same
  log and projection as every record, and the store's walks read them
  through the same engine seam.
- It does not widen what Lakelands reserves by roster
  (`docs/decisions/003-reservation-by-namespace.md`): link types are
  recognised by their name, `sys.link.` and the relation, as every
  system type is.

## Extended: what the code settles

- **Reading links.** Every engine answers a link relation's edges by one
  rule, `lakelands_core::links::link_edges`, over the live link records
  at the view's position. At the head, SQLite and PostgreSQL find the
  candidates through the exact-text index they already keep on `from`
  and `to`. At an earlier position they read the link type's records
  there. The engine seam and the projections' schemas are unchanged.
- **A relation's fields follow 027.**
  - An added optional link field is a declared change: the owner type
    is declared again, and its link type with it, in the same append.
  - A change that reshapes link records (a retyped field, a relation
    retargeted, or removed and declared again with another target) is
    refused at `declare` and goes through `migrate` on the owner type.
    Its preflight reports the link records beside the type's, each field
    named with its relation, and `migrate_apply` migrates each changed
    link type in the same append.
  - A link field is never renamed: a rename is a drop plus an add.
  - The link type is classified at every declaration, against the one in
    force. A relation dropped by `migrate` and declared again with
    another target, or with a field retyped, would reshape the links it
    kept, so it is refused, naming the link type, and goes through
    `migrate`.
  - Narrowing a relation's cardinality is a migration. Its preflight
    counts the live links that would break the narrower rule, and
    refuses while any would.
  - Removing a relation is a migration. Its preflight lists the relation
    under `dropped` with the number of its live links, as it lists a
    removed field with its values
    (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
    extended); the links stay as history.
  - The link type is checked as any type is, when its owner is declared:
    its fields' kinds, functions and embeddings. A field that cannot work
    fails at `declare`, not at the first write.
  - A relation removed from its type keeps its link type and its records
    as history, and its link doors refuse it.
- **A relation's authority** may change by an ordinary declaration: it
  swaps the door the links arrive by and keeps the links already
  written. A `source` relation's own fields are `source` (or derived):
  an `authored` field on a link no door writes is refused at
  `declare`.
- **`unlink`** removes a link whatever the relation's authority, as
  `retire` removes a record whatever its fields' authority. It is
  governed by the principal's grants on the link type. Ingest writes a
  source relation's links, but cannot retire one. A principal who is not
  shown the type that declares the relation is shown nothing of its link
  type, at these doors and at every other
  (`docs/decisions/006-grants-not-roles.md`, extended).
- **Retiring an end** keeps its links, as retiring a referenced record
  keeps the references (032). The walk does not reach the retired record.
- **A restricted link** is walked only by a principal holding the
  `restricted` grant. An edge's fields are those of the link type the
  principal may read: a link's edge comes from its own record, not from
  the records it joins.
- **Tools and rights.** `describe` names, per type, the relations whose
  links the principal may write: those whose links are `authored` and
  whose link type's `from` and `to` the principal may write. The tool
  generator makes `<type>_<relation>_link` and `_unlink` for them, and
  checks their names for collisions with every other tool. A link type's
  own description offers `find` and `get`, never `upsert` or `retire`.
- **A trigger on a relation** watches its link type: `on` names
  `sys.link.<type>.<relation>`. It fires on every write of a link, as a
  trigger on a type fires on every write of a record: `link`, a link
  written again, `unlink`, and each link a source ingests, per window.
  Its guard reads the link's fields (`from`, `to` and the relation's
  own), and `changed(f)` holds on a write that changes `f`. On `unlink`
  the guard reads the link's fields as they were, and none reads as
  changed. A host tells a link from an unlink by the write the
  `trigger_fired` event's `cause` names: `record_upserted` or
  `record_ingested` against `record_retired`. A relation carried by a
  reference field has no link type, since its links change with the
  field: a trigger naming one is refused on `on`, pointing at
  `changed(<field>)` on the type. The trigger declaration gains no
  member, so the log format does not change.
- **The WIT package** gains the two doors, `relation-decl.authority` and
  `type-rights.link`, and stays at `lakelands:data@0.2.0`, which is not
  released yet.

## Extended: a relation may instead be derived

A relation may have no carrier at all: a derived relation connects
records by matching values when the store reads, and stores no link
(`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
The carrier rule holds across all three kinds: a relation carried by
`via`, by link records or by matching values keeps that carrier.

## Extended: the order of refusals (lakelands#338)

The link doors refuse in this order, in the frame every write door
shares (016, extended): the lake, the context, the door's own checks,
the append.

**`link`**: the type is declared and shown to the caller
(`not_found`), and so is the relation (`invalid`), which alone give way
to a retry: a call sent again under its key is answered as the first
was, though the type or the relation is no longer there for its caller;
it is not a view (`invalid`, 018, extended); it declares the relation,
carried by link records (`invalid`); the relation's links are authored
(`wrong_door`); `from` and `to` are not among the fields (`invalid`,
naming the field); the retry, a retry that itself fails included; the
record checks of 016 on the link record; the relation's
cardinality over the live links (`constraint`, naming `to`), after the
record's grants so that a principal who may not write the link learns
nothing of the links there are (024, extended).

**`unlink`**: the type and the relation are declared and shown, or the
call is a retry, it is not a view, it declares the relation carried by
link records, as for `link`; then the retry and
`retire`'s checks on the link record: its grants (`not_granted`), the
link is live (`not_found`), the expected version (`conflict`). An
unlink is not refused for the relation's authority (above).

A view declares no relation carried by link records, and an undeclared
type is no view, so the steps from the type to the relation answer
`not_found`, then `invalid` naming the type, and are stated, not
checked, between themselves.


## Extended: a renamed relation keeps its edges (lakelands#429)

A relation's name is part of a type's shape, and names change. A relation
that disappeared under one name and appeared under another was a removed
relation and a new one: its links stayed behind under the old link type.

A migration now names the rename, `relation_renames`, old name to new:

- **The edges are the same edges.** A relation carried by a `via` field
  or derived from matching values has no record of its own, and is named
  anew. One carried by link records moves them: at the `migrated` event
  the fold takes every record of the old link type to the link type of
  the new name, under the same key, with its version, fields, provenance
  and flags, retired and restricted ones too. No event is written for a
  link. The old link type's keys end there, so a read as of before
  answers the links under the old name, and a set's memberships move
  with the records.
- **What names the old link type is carried.** A trigger or a verb on
  it, an object set of it and a view that reads it are declared again by
  the lake under the new link type, in the migration's append, after the
  new link type's declaration, and the preflight's report lists them. A
  relation's `inverse` is its own member and is stated in the new shape.
- **A rename moves and changes nothing.** The carrier, the target, the
  cardinality, the link fields and who writes them stay; a migration
  that also re-keys records is refused, as is one that changes the link
  fields, each naming the relation. A rename onto a name whose link type
  still holds the links of an earlier, removed relation is refused, and
  so is one whose link type a reference field or a relation of another
  type names: that type is named to an author who is shown it, and
  another is told that a declaration refers to it.
- **A link type no relation carries takes no link.** The old link type
  stays declared, for the reads as of before, and so does a removed
  relation's. An ingest into one is refused, naming it, and a verb
  declared on one is refused as on a type that is not declared
  (lakelands#759); a verb in force moves to the new link type with a
  rename. Where a renamed
  relation takes a name whose link type stands, empty, in another shape,
  that link type is declared again in the shape of the links it takes;
  a declaration that reads a field of it which the renamed relation
  lacks refuses the rename, as it refuses a migration that takes a field
  away.
- **Grants follow the link type** as they follow a field
  (`docs/decisions/006-grants-not-roles.md`, extended): a restriction in
  both directions of time, a permission never.
- **The log's format** takes `relation_renames` as a member of a
  `migrated` event; a migration that renames no relation leaves it out.
