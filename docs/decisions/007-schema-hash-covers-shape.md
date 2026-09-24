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
