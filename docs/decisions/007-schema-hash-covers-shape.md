# 007 — The schema hash covers shape, never meaning

## What is decided

Every type has a schema hash computed over its shape: the fields with
their types and whether they are required, the key, the relations and
their targets, and the structure of a derived type. Anything that
changes how a record is stored or validated is shape, and changing it
changes the hash.

What a field means is not shape. Descriptions, labels, categories,
display formats, presentation marks, and the declared write authority
of a type or field are excluded from the hash. Relabelling a field,
rewriting its description or changing who may write it leaves the hash
byte-identical.

## Why

The schema hash decides when data has to move. A changed hash means a
migration: a preflight over the stored records, a conversion, a
rebuilt projection. If meaning marks were part of the hash, every
wording fix would look like a migration, and people would stop
improving descriptions to avoid one. Agents read those descriptions to
choose their tools, so they have to stay cheap to change.

The hash is also half of an as-of address: a schema hash with a log
position identifies exactly what shape a reader saw. For that it must
change when and only when the shape does.

## What it is not

- It does not make meaning marks unversioned. The type's version counts
  every revision, a changed description included; the hash identifies
  the shape, the version counts the changes.
- It does not make every non-hashed change harmless. A change of write
  authority affects who may write next; it just never reshapes a
  stored record, so it needs no migration.
- It is not a hash over the declaration text. Reordering a declaration
  without changing its shape does not change the hash.

## Extended: exactly what the hash covers

`docs/log-format.md` §2.2 now defines the hashed object member by
member, so two implementations compute the same hash. Four choices in it
follow from this record's rule and are stated here so the rule stays the
reason:

- A field's whole type (kind, value type, reference target, vector
  length) is shape: changing any of them changes how a value is stored
  or validated.
- A view's conditions are hashed in their typed form, so writing the
  same view as text or as a typed expression is not a change of shape.
- A relation's `via` and `inverse`, a field's `searchable`, and a type's
  `valid_from` / `valid_to` are excluded. None of them changes what a
  stored record holds or which values it accepts: `via` and `inverse`
  name how a relation is walked, `searchable` decides what an index
  covers, and the valid-time fields name which existing fields a read
  filters on. Changing them rebuilds indexes at most, never a record.
- Declared authority and `depends_on` / `on_input_change` stay excluded,
  as above: they govern the next write, not the stored shape.

A type's `language` (the tag full-text search stems by) is excluded for
the same reason as `searchable`: it decides how an index is built, never
what a record holds.

The hash is one function, `lakelands_api::shape::schema_hash`, with the
expression parser and the canonical bytes beside it in `lakelands-api`:
a host that keeps declarations in force computes the hash of its own
declaration with the function the store uses, and the mock calls it too,
so no second copy of the allowlist can drift from it.

## Extended: a proposed shape

A declaration or a migration that is proposed moves no schema hash: the
hash a type answers changes at the acceptance, where the declaration is
logged, and a rejected proposal leaves it as it was
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended).

## Extended: a host's annotations are meaning

A type, a field and a relation may carry `annotations`, the host's own
marks by namespaced key (`acme.icon`, `acme.package`): presentation,
policy flags the host enforces itself, provenance. They are meaning,
never shape. The hash is an allowlist of shape members, so annotations
stay out of it without a rule of their own. A suite check holds this
byte for byte, so that a member added to the allowlist later cannot
pull them in unnoticed. A type whose annotations alone change keeps its
schema hash, and its version counts the change like a changed
description. Carrying annotations in the log is a new member, so it is
format version 5 (`docs/log-format.md` §6).
