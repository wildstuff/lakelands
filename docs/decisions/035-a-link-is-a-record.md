# 035 — A link is a record

## What is decided

A relation declared without `via` is carried by **link records**: one
record per linked pair, of a type the store derives from the relation,
`sys.link.<type>.<relation>`, keyed by the pair (`from`, `to`) and
holding the relation's own fields. `docs/log-format.md` §2.3 specifies
it. Links are written and removed by the record events the log already
has (`record_upserted`, `record_ingested`, `record_retired`); no event
kind is added, and the log format stays at version 1.

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
ordinary doors refuse its records, so a link is only ever written
through the doors that check the relation's authority and cardinality.

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
