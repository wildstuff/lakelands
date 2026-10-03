# 006 — Grants, not roles

## What is decided

The store knows principals and grants, never roles. Every call carries
one `Principal` (kind, id, lake, grants), and the grants say, per field,
what that principal may read and write. The store enforces them on every
read and every write: a field the principal may not read is masked or
absent in the answer, and in the schemas the generated tools expose; a
write carrying a field the principal may not write is refused whole.

Turning a host's identities, roles, teams or policies into grants is the
host's job, done in its `PrincipalResolver`. Lakelands' own default
resolves an API key or a static configuration entry to one principal.

## Why

Roles are policy, and every organisation models them differently. If the
store understood roles, it would have to adopt one model and every host
would have to bend to it. Grants are the common denominator every role
model compiles to, and they are what the store actually needs to decide
a single read or write.

Enforcement has to sit in the store because a rule enforced anywhere
else is skipped by a caller that reaches the store directly. Per-field
grants are also what lets two writers share one record safely (see 012):
each writes only the fields it is granted.

## What it is not

- It is not an off switch. There is no mode in which the store stops
  checking grants; a fully trusted principal is one whose grants cover
  everything.
- It is not authentication. Who the caller is gets decided before the
  store is called, by the resolver.
- It is not partial success. A write is never applied with the
  forbidden fields silently dropped.

## Extended: `none` hides a field, `masked` withholds its value (lakelands#394)

The two grants that withhold a value say different things. `masked`
tells the principal that the field exists and withholds its value:
the field is listed as masked, described by its shape, and a read that
probes it or a write that carries it is refused as not granted, naming
it. `none` says the field does not exist for that principal. It is
absent from answers, from the masked list and from metadata, from
`describe` and from every generated tool. A probe or a write that names
it gets the answer for a field the type does not declare: the same
kind, the same sentence, the same field named. Any other answer would
let a single refusal reveal what every answer hides. A search that
names no fields reads only the searchable fields the principal knows
of.

`sensitive` raises only an *inherited* `none`, from the type's or the
lake's default for a field the grants do not name, to `masked`, so a
field nobody thought about is shown as withheld rather than silently
dropped. A `none` given to the field by name always hides it, and so
does a `none` that a field built from another inherits from its source
(024).

## Extended: write without read (lakelands#393)

A host that acts for a model must keep some values from the model, and
it does so with the model's read grants. Withholding the read must not
withhold the write: a tool the model calls may carry a whole record
from a source, that field included, without the model ever seeing the
value. Until 0.8.0 access was one ladder, `none < masked < read <
write`, so a grant that withheld the read withheld the write too.

A field may now be granted `write_only`. For reading it is as `none`:
absent from `fields`, `masked` and `meta`, from every read schema, from
`describe`'s derivation and meaning, and from the names a host's watch
shows. For writing it is as `write`: `upsert`, `ingest` under its
door's authority, a verb's effect, a proposal, and `retire` when every
other field is writable too.

- **Nothing flows back.** A write answers what it answers today, its
  position and version, never the value or the field's provenance, and
  a retry answers the same. A refusal of the write may name the field
  the caller sent; it never shows a stored value.
- **A probe is not granted.** A write-only principal knows the field
  exists, so a filter, order or search on it is refused `not_granted`,
  naming it, as for any field the principal may not read; it is not
  answered as an undeclared field.
- **The schemas say it.** `describe` lists the field with its access
  `write_only`, and the generated tools put it among the writable fields
  of `<type>_upsert` and of `propose`'s record writes, and in no read
  schema.
- **Access is not ordered.** No code decides access by comparing
  values; it asks whether a grant reads, writes, or appears in a read
  answer. A host that matches on the access enum gains one arm.

