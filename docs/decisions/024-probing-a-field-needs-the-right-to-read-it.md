# 024 — Probing a field needs the right to read it

## What is decided

A read that filters on a field, orders by it, aggregates over it or
searches it is refused when the principal may not read that field —
also when the field would have been masked in the answer. The refusal
names the field. Where the principal may not know of the field at all (its
access is `none`), the refusal is the one for a field the type does not
declare (006, extended in lakelands#394). A read at `valid_at` filters on its
type's `valid_from` and `valid_to`, so it probes them as a filter on
them would: `find`, `get`, `search`, a pattern's start and every type a
walk reaches (lakelands#372).

A search probes every searchable field of its type, in every mode, and
the vector field it names. A vector embedded from text encodes that
text, so a vector or hybrid search also probes every field its vector
is embedded from, even when the principal may read the vector itself.
A hybrid search that one of these refuses is refused whole, never
answered from its other half. This tightened in 0.5.0 (lakelands#242).

A lexical or hybrid search may name the searchable fields it matches in
(`fields`). It then probes those fields instead of every searchable one,
so a searchable field the principal may not read no longer refuses a
search that does not name it. Only the text half narrows: the vector of
a hybrid search still encodes every field it is embedded from, and is
probed as above. A named field the principal may not read is refused as
not granted before its searchability is judged, so the refusal never
tells whether a field it may not read is searchable; a named field that
is not declared or not searchable is invalid (lakelands#236).

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
- It does not decide what a writer may write. A write is governed by
  the principal's write grants (006, 012); what a write's refusal may
  tell is extended below.

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

## Extended: a write's refusal tells only what its grants cover (lakelands#343)

A refusal is an answer too. A write that would break a rule about the
lake's data, refused before the writer's grants are checked, tells a
principal without those grants what the data holds: that a record
already links somewhere, and to which key; that a type exists and has a
field. So the line this record draws for reads holds for writes:

- **Grants come before anything that reads the data they protect.**
  `link` checks the link record's grants, with the rest of the record's
  own checks, before it reads the relation's live links for its
  cardinality. `add_field` checks the `schema` grant before it looks the
  type up or asks whether it has the field.
- **The version comes right after the record is found,** before
  anything is judged on that record. A verb's call compares the expected
  version with the record before `requires` is evaluated on it, so a
  caller with a stale version is told its version is stale, never that
  the record fails a condition on a state it has not seen. This is how
  `retire` already ordered it.

What a write checks is unchanged; only which refusal comes first.

## Extended: a field built from another is read as that field (lakelands#387)

A derived field hands out what it is computed from: `g = salary * 2`
is the salary, doubled. An embedded vector encodes the text it is
embedded from. So a field is read no further than every field it is
built from, its `depends_on` and its `embed.from`, and theirs in turn.
A source the principal may not read hides the field. A masked source
masks it, unless the field is already hidden. The rule lives in one
place, the field's access (`lakelands_core::rights::field_access`), so
it holds on every door: such a field is absent or masked in `get`,
`find` and a pattern's rows, and in `describe` and the tools.
Filtering, ordering or aggregating over it is refused as not granted,
naming it, as for its source. A search over a vector embedded from a
field the principal may not read still names that source, as above.

## Extended: a hidden field is not removed by one who may not know of it (lakelands#475)

A field that does not exist for a principal on every read door is not
reached by that principal's change of shape either: its migration and
its redeclaration are read as its view of the type, and the hidden
field is carried over as declared
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended). The preflight's report names no such field to it, and a
proposer reads its own proposal without it.

## Extended: a view's own probe names nothing (lakelands#505)

A read of a view probes what the view's definition reads: its `where`,
its grouping and what its measures aggregate, in its source and in each
merged one. A reader who may not read one of those fields is refused, as
for any probe. But the field is one the view named, not the reader, and
the answer for a field the reader names itself ("`invoice` has no field
`salary`", or not granted on a masked field) would tell it what the
definition reads. `describe` already withholds that definition from such
a reader. So the read is refused as **not granted, on no field**, and its
text names neither the field nor the source: the view is one this reader
may not read. `find`, `get`, and a pattern that starts on the view or
steps onto it answer alike.

A principal with the `schema` grant is shown every definition, so the
field's name tells it nothing new, and it is the one who can act on it:
it keeps the answer that names the field.

## Extended: a proposed value is shown where the record's is (lakelands#499)

A proposal holds values before they are stored: an upsert's patch, and a
call's payload, which is what the verb's fields become. `proposals` used
to show both whole to every reviewer and to the proposer, so a principal
that holds `review` read the proposed value of a field every other door
hides from it. The listing now shows them as a read of the record would:

- **A patch** keeps the fields its reader may read. A field the reader
  may not know of leaves it, and so does a masked one: a patch holds
  values and has no place for a name without one.
- **A payload** is shown whole to a reader who may read every field the
  verb sets, and empty to any other. Showing it member by member would
  need the payload paths each assignment's value reads, which the
  declaring rule does not hand out (lakelands#470). For a verb that
  reaches outside the lake, the payload goes to the executor and is
  shown to a reader who may see the verb's type.
- **A write to a type the reader may not see** is listed without its
  write: the type's name and the key are names. The entry stays in the
  listing, so that a principal holding `review` and no read grant still
  finds what it is asked to decide.

The proposer reads its own proposal by the same rule, whole for as long
as it may read what it wrote.

The names follow the same reader. A proposal on a record it is not
answered (one restricted, without the grant) is listed without its
write too. A migration's report, a rename and a proposed declaration
are shown by what the reader may know of, and an entry that holds more
than its reader is shown says `more`
(`docs/decisions/013-verbs-run-by-declared-risk.md` and
`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, each
extended).
