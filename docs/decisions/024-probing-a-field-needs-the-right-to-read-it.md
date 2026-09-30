# 024 — Probing a field needs the right to read it

## What is decided

A read that filters on a field, orders by it, aggregates over it or
searches it is refused when the principal may not read that field —
also when the field would have been masked in the answer. The refusal
names the field.

A search probes every searchable field of its type, in every mode, and
the vector field it names. A vector embedded from text encodes that
text, so a vector or hybrid search also probes every field its vector
is embedded from, even when the principal may read the vector itself.
A hybrid search that one of these refuses is refused whole, never
answered from its other half. This tightened in 0.5.0 (lakelands#242).

`describe` holds to the same line: a masked field is described by its
shape only — its name, type, access, whether it is required and
sensitive, its authority and what an input change does to it — never by
what it is computed from or what it means, and a function, trigger,
object set or view that names a field the principal may not read is not
described to it. This tightened in 0.4.0 (lakelands#206).

## Why

Masking the answer does not protect a value the question already
revealed. A filter `salary > 100000` answers, record by record, whether
the hidden salary is above the line; a sum over a group of one is that
record's salary; an ordering ranks the hidden values. Each of these
turns a masked field into an oracle. Refusing the question is the only
answer that leaks nothing.

## What it is not

- It does not refuse reads of records that merely have such a field. A
  record is still returned with the field masked or absent.
- It is not a rule for writers. A write is governed by the principal's
  write grants (006, 012).

## Extended: annotations, and a relation's link fields

A field the principal may not read is described without its
annotations, as without its description and label. This holds for a
type's own field and for a relation's link field alike.

`describe` answers a type's relations with their link fields. Until
now it answered each relation whole, so a link field the principal may
not read still showed its description and label. That was a gap in the
answer this record tightened in 0.4.0, and it is closed in 0.5.0
(lakelands#239): such a link field is described by its shape alone,
without its derivation, meaning or annotations. A type's and a
relation's own annotations are shown to whoever the type is described
to, as their descriptions are.

An annotation grants nothing and restricts nothing. What a principal
may read is decided by its grants alone. A host that marks a field with
a policy of its own enforces that policy itself.
