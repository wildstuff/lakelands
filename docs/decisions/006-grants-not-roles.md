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
names no fields reads only the searchable fields the principal reads
(lakelands#662), so a masked one neither refuses nor ranks it.

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

## Extended: a grant holds a field through its renames (lakelands#527)

A grant names a field, and a migration can rename one. The grant stayed
on the old name and the field fell to the type's default: a principal
held at `none` or `masked` on `holder` read it whole as `keeper`, with
nothing but the rename having happened. A host that renamed its grant
with the field was answered the old name's value in every read as of a
position before the rename.

> A field's access is the lowest of what its grants say under every name
> the field bears, has borne, or, for a read as of an earlier position,
> will bear. A grant reaches further than its own name only to take away.

- **What a name says.** The name the field bears at the position read
  says what it always said: its grant by name, else the type's default,
  else the lake's. Any other name of the field says something only where
  the grants name it. A name they do not name says nothing, or a default
  would undo a grant on the present name.
- **The lowest of two accesses** judges reading and writing apart: it
  reads the value only if both do, shows the field masked if both at
  least show it, and writes only if both write. So `write` with
  `write_only` is `write_only`, and `read` or `masked` with `write_only`
  is `none`.
- **A restriction follows the field, in both directions of time.**
  `holder: none` holds for `keeper`, for `owner` after a second rename,
  and `keeper: none` holds for `holder` in a read as of before.
- **A permission stays with its name.** A principal granted `holder:
  write` on a type it may otherwise not see loses the field at the
  rename; its host names the new name to give it back. A host that
  widens a renamed field names the new name and drops the restriction on
  the old.
- **The result is a grant by name** on the name the field bears at the
  position read, put where the principal enters, before any door asks.
  So every place a field's name meets a grant holds it, and a
  restriction that followed counts as one given by name: a `sensitive`
  field held at `none` under a former name is hidden, not masked.
- **A migration that is not in force** is judged with its own renames as
  the last of the list: a proposed rename is listed to a reader kept
  from the old name with neither the pair nor the new name, and a
  migration's answer names in `renamed` each rename the one answered may
  know of. A proposal that waited while the lake renamed a field or a
  relation is listed by the names it was written with, the reader's
  grants followed to the proposal's position: a restriction on a name
  taken since reaches the name the field bore then. So a reader kept
  from the field, or from the relation's link type, under the name in
  force is listed neither the pair, nor the proposed name, nor a field
  the proposal keeps under its old name, in its shape or in its hashes
  (lakelands#594, lakelands#644).

**Two costs, taken with the rule.**

- **Two fields that have borne one name cannot be told apart by that
  name.** After a swap of `a` and `b` each of the two names restricts
  both fields; after `holder` became `keeper` and a new field took
  `holder`, a grant on `holder` restricts both. A host tells them apart
  by a name only one of them has borne: it renames one once more.
- **The list holds renames, not removals.** A field that is removed, and
  whose last name a new field takes and is later renamed from, has its
  chain of names continued by the new field, which is then restricted by
  the removed field's former names too. It errs to the closed side.
  Where several chains end in the name a rename takes, the rename
  continues the one whose last name was taken latest: a name is free
  when a rename takes it, so every earlier bearer of it is gone, and a
  field renamed onto a name another field bore is held by its own names.

**Where the rule is applied, and when.** To the principal, where it
enters: a read takes the list from the view it reads at the head, so a
rename is in its shape and in its grants or in neither; a read as of a
position takes the head's list, where a later rename can only take away.
A write is followed before it waits for the writer, and a migration can
rename a field while it waits. Where the list has grown when the write
takes the writer, it is refused to be sent again; an ingest is followed
under the writer itself.

**Where the renames are kept.** A rename happens only in a `migrated`
event. The fold keeps the lake's list of them, each with its position
and type, as one recorded output under a seam name of the store's own,
`sys:renames` (`docs/seams.md`); it is no event, and no door answers it.
A rebuild and an import fold the log and make the list again. A lake
made before this holds no list: its first open finds one from what the
projection keeps, without reading the log. A type's declaration is in
force since the event that put it, so for each type past its first
version the one event at that position is read, then the type as of the
position before it, down to its first version. The handle that writes
records the list; one opened read-only before any writer has opened
keeps it in memory.

**A type's name follows the same rule (lakelands#429).** A renamed
relation carried by link records moves them to the link type of its new
name, and a grant can name a link type. The list holds an entry of a
second kind for it: the position, the link type's old name and its new
one. The step over a type's names comes before the step over fields:
the grants on every name the type has borne or will bear become one
entry under the name it bears at the position read, a field's access
there the lowest of what that name's entry says (its grant, else its
default, else the lake's) and of what each other name's entry says for
it (its grant, else its default). A grant that is only a `default` is
followed too, so a principal is followed whenever its grants name a
type. Each chain of names reads the grants as they were given, and of
the chains that end in a name the one that took it last continues.

## Extended: a type made of another is shown with it (lakelands#595)

Two kinds of type have no records of their own making. A link type is
the edges of one relation: its name spells the type that declares the
relation and the relation, and its records are that relation's links
(`docs/decisions/035-a-link-is-a-record.md`). A view is its sources'
records, computed (`docs/decisions/018-one-pattern-query.md`). Each was
judged by the grants on its own name alone, and those the grant on the
other type does not touch: a principal kept from `invoice` by
`{default: none}` was told that `invoice` does not exist, and was
described `sys.link.invoice.approved_by`, read its records, wrote and
retired its links, and read through a view how many invoices there are.

**A type made of another is shown to a principal only where that other
type is**: a link type where the type that declares its relation and
that relation's target are (lakelands#662), a view where its source and
every merged source is. One who is told a type
does not exist is told the same of what is made of it.

- **One function decides it**, the one that decides whether a type is
  described at all. So `describe` and the tools built from it,
  `describe_type`, `find`, `get`, `search`, a pattern's start and each of
  its steps, a set or a trigger on the type, a proposal's listing and
  every refusal that names a type only to one `describe` shows it to
  follow it without a rule of their own. The answer is the one for a
  type that is not declared, never a refusal: a refusal would tell that
  something is there.
- **A grant on its own name does not give it back.** The grants on a
  link type's name and on a view's name stay, follow a rename as before,
  and can take more away; they cannot show what the other type's grant
  withholds.
- **Shown means what it means for any type**: the principal has some
  access to the type or to one of its fields. A principal who reads one
  field of `invoice` is shown its link types and the views over it,
  each under its own grants, and a view still refuses the reader who
  does not read a field its definition probes
  (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
  extended).
- **It runs down a chain**: a view over a view over a link type is shown
  where the type that declares the relation is.
- **`link` and `unlink` ask it before they name a relation.** They reach
  a link type by its owner's name, so they ask whether the caller is
  shown the owner, and answer one who is not as for a type that is not
  there, whether the type has the relation or not. Its retry of a write
  it made is answered that write first, as every retry is before the
  grants are asked: it tells the caller its own write.
- **Every door that writes a record asks it first (lakelands#607)**:
  `upsert`, `retire`, `propose` of either, `ingest`, and a verb's call
  for the type the verb is on
  (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
  extended).
- **A relation is named from its target's end only to one shown its
  owner.** A relation and its inverse are declared on the owner, so a
  step from the target by the inverse, or by the relation's own name
  walked in, is answered one who is not shown the owner as a relation
  the target does not have, for a relation carried by links and by a
  reference field alike.
- **A relation is declared to a principal only where it is shown both
  its ends (lakelands#662)**, and, for a derived relation, may know of
  every field its match pairs, on both sides. A relation to a target
  that is not declared was already left out; describing one to a target
  the principal is not shown would tell it the target exists, and the
  link type's description and its `to` field name it. One it is not
  shown is answered at every door as a relation the type does not
  have, naming only what the caller sent: `describe` and `may.link`, a
  step out, in, either way or by the inverse name, a graph and a
  measure, whose edges leave it out, `link` and `unlink` after a retry
  is answered, a proposal's listing and its report, and the hash of the
  shape shown. A match over a field the principal sees masked stays
  shown: the field is known, and a walk over it is refused as not
  granted, naming it. An author who declares a type again from its view
  keeps such a relation as declared, at its place, as it keeps a hidden
  field (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
- **A pattern's probes ask it too (lakelands#653, lakelands#654).** An
  order, a measure or a grouping over a path `type.field` on a type the
  principal is not shown reads nothing and is left alone, as a path on
  a type that is not declared is. A step over a derived relation probes
  the matched fields of the relations it walks and of no other: its
  own out of the type it leaves, and one into that type whose owner
  the principal is shown. A relation a hidden type derives under the
  same name reaches no record the principal is shown and is not probed,
  so no refusal names it.
- **A link type is made of its relation's target too (lakelands#662).**
  It was made of its owner alone, and a principal kept from the target
  read the links and their `to` keys. Its `to` names the target, so it
  is followed as a view's source is.

For a host: a principal that reads or writes a type's links is granted
something of the type and of its relation's target, and a principal that reads a view is granted
something of each of its sources.

## Extended: `sensitive` shows a field, never a type (lakelands#608)

`sensitive` raises a field nobody named from an inherited `none` to
`masked` (lakelands#394). That sentence was written for a field in a
record its reader holds, and the function that decides whether a type
is shown counted the raised field as access to the type. So a principal
its host gave nothing of `invoice`, by `{default: none}` or by a lake
default of `none`, was described `invoice` with `note` masked, read
every invoice's key and version, was answered its writes field by field
instead of as on a type that is not declared, and through a grant on a
link type or a view read every link and every count. Marking a field
`sensitive` made its type more visible than leaving it unmarked, and a
host could hide the type only by naming every sensitive field `none`
for every principal.

**A type is shown to a principal by what its grants give it: some
access to the type, or to one of its fields. `sensitive` raises a field
to `masked` only for one who is shown the type; it never shows a
type.**

- **The one function asks the access before the raise.** Everything
  that asks whether a type is shown asks it, so the description, the
  reads, the doors that write a record, the link doors, the settings'
  judge and what is made of the type follow without a rule of their own.
- **What stays.** Inside a type its principal is shown, a sensitive
  field nobody named is listed as masked, and a filter on it is refused
  as not granted.
- **A field is counted as it is read.** A field built from another is
  read as that field (024), so a grant to read a field, or to see it
  masked, whose source is `none` gives nothing and shows no type. A
  grant to write it leaves it write-only (lakelands#393), and that
  shows the type.

For a host: a principal that is to see that a field is withheld is
granted something of the type, or the field as `masked` by name.

## Extended: a type that is shown is one its principal may find and get (lakelands#620)

`describe` said `may.find` and `may.get` only of a type its principal
reads a field of. The read doors never asked that: a principal shown
`invoice` with every field masked, with one field masked by name, or
with a field it may only write, was answered every invoice by `get` and
`find`, counted them and walked from them, while the description said
it may not, and the tools built from the description offered it neither
read nor named the type in the pattern, search and subscribe tools.

**A principal that is shown a type may find and get its records, and
`may.find` and `may.get` say so of every type `describe` answers.**

- **A record is its key, its version and what the grants show of each
  field**: the value of one that is read, the name of one that is masked,
  in the declaration's order, and nothing of one that is hidden or only
  written. One who reads no field is still answered the record; that is
  how it sees what is withheld, and what a writer writes against.
- **The probe is refused as before.** A filter, an order, an aggregate
  or a search over a field the principal does not read is not granted
  (024), so `may.find` promises the door, not every question.
- **A key is shown to whoever is shown the record.** A key made of
  fields holds their values. So a type keyed by a field its principal
  may not know of is not shown to it (lakelands#715): it answers as a
  type that is not declared, and so does every type made of it, a link
  type of its relations and a view over it. A key field the principal
  sees masked or may only write still gives its value through the key:
  the field is known, and only its value is withheld elsewhere.
- **A step walks a relation by its owner.** A relation is declared on
  the type the principal is shown, so a step over it is answered the
  other end's key also where the principal's grants keep it from the
  relation's link type; it is then shown no link field.

For a host: a principal that is not to list a type's records is granted
nothing of the type. Holding a key field at `none` now hides the type;
before lakelands#715 it hid the field and answered its value as the
key.

## Extended: a type kept from a principal hides what refers to it (lakelands#718)

A reference to a type its principal is not shown is a field it may not
know of (024, extended), so the made-of rule follows it: a type keyed by
such a reference is not shown (lakelands#715), nor a view grouped by one,
nor a type keyed by a reference to either, to a fixed point, and a
relation carried by one is not shown (lakelands#727). A host that keeps
an agent from a type keeps it, by that alone, from every type made of
it through a key. To show such a type, it grants the target type, its
key fields at least masked; an unkeyed type can be granted with nothing
else.


## Extended: nothing beside a type names it to one it is kept from (lakelands#770)

A type its principal is not shown is not named by what is declared
beside it either. A verb on such a type is not described and no tool
is made of it, even where the principal could write every field the
verb sets: `call` and `propose` already answer it as a verb that is
not declared, and `describe` now agrees with them. A function is shown
to a principal only where `describe` lists it (every type that computes
a field by it is shown to the principal, which reads that field and each
input, or it holds `schema`). The conditions of a value type's rename name
such a function only to a reviewer shown it; to any other it is told
only as one there may be (`conditions_unshown`), as a set or a trigger
it is not shown is.

## Extended: a hold now hides a record at every position (lakelands#790)

A retention hold is about the record, not the moment. To a principal
without the `restricted` grant, a record held at the head is not there
at any position: a read as of an earlier position answers it as a record
that never existed, at `get`, `find` (its rows, `total` and aggregates),
a view computed or kept, a walk, a pattern and a search, lexical, by
vector or both, whose candidates, ranks and counts leave it out. A
record held then and released since is answered there as any record is,
since the hold that stands now is the one that governs.

A read as of a position by such a principal reads through a view that
gives each record its hold at the head, asked of the head one page of
keys at a time. A record is found at the head by the name and the key
it bears there, after each rename of its link type and each re-key
since, and one the head does not hold keeps the hold it had: a record
is never released by not being found. Every door then judges the hold as it does at the head.
A page the engine fills with records held now is read on until it is
full, a total is counted in pages, and a kept view, folded as it stood
then, is computed instead, and `describe` says it is not kept here. A
read at the head, or by a principal with
the grant, is unchanged.

**Why.** A hold withholds a record from those without the grant. A read
as of the moment before the hold would hand it back whole, so a hold
that cut only the head would withhold nothing. A record released from a
hold is one its readers may see, and the history before its release is
part of it.
