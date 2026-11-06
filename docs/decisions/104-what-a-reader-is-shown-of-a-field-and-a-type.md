# 104 — What a reader is shown of a field and a type

These rules are what a reader is shown of a field and a type: the one
access a field has, over every name it has borne and everything it is
built from; what each grant answers of it at every door; what a type's
grants give of it, of what it is made of and of what it names; and the
description, the schema hash and the report's id made over the shape that
reader is shown.

These rules are one family of the decision in `006 § A call carries one
principal with grants, and the store checks every read and write`, stated
and changed there until lakelands#1768 moved them here word for word.

## What is decided

### A field has one access, the lowest over every name it has borne and everything it is built from

#### A field may be granted `write_only`: written, and never read

A host that acts for a model must keep some values from the model, and
it does so with the model's read grants. Withholding the read must not
withhold the write: a tool the model calls may carry a whole record
from a source, that field included, without the model ever seeing the
value. On one ladder of access, `none < masked < read < write`, a
grant that withheld the read withheld the write too.

So a field may be granted `write_only`. For reading it is as `none`:
absent from `fields`, `masked` and `meta`, from every read schema, from
`describe`'s derivation and meaning, and from the names a host's watch
shows. For writing it is as `write`: `upsert`, `ingest` under its
door's authority, a verb's effect, a proposal, and `retire` when every
other field is writable too. A type its principal may write and not
read, and one keyed by a field it may only write, is not declared to a
read and is described for its writes alone
(`docs/decisions/092-reading-is-judged-apart-from-writing.md`).

- **Nothing flows back.** A write answers what any write answers, its
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
  answer.

#### A grant holds a field through its renames

The right to read a field is asked by the field's name, and a migration
can change the name. A field a principal may not read is not read under
the name a rename gives it, nor under the name it had in a read as of
before. A grant that stayed on the old name let the field fall to the
type's default: a principal held at `none` or `masked` on `holder` read
it whole as `keeper`, with nothing but the rename having happened. A
host that renamed its grant with the field was answered the old name's
value in every read as of a position before the rename.

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
  the proposal keeps under its old name, in its shape or in its hashes.

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
A write is followed at its turn in the lake's queue of writes, before it
takes the writer: a migration through the same handle waits its turn as
the write does, so a write that waited behind one is followed through its
renames. Only what the writer catches up with as it is taken can rename
a field between the two: another handle's migration, or one whose fold
the lake has not yet made; where the list has grown when the write takes
the writer, it is refused to be sent again. An ingest is
followed under the writer itself.

**A type's name follows the same rule.** A renamed relation carried by
link records moves them to the link type of its new name, and a grant
can name a link type. The list holds an entry of a second kind for it:
the position, the link type's old name and its new one. The step over a
type's names comes before the step over fields: the grants on every name
the type has borne or will bear become one entry under the name it bears
at the position read, a field's access there the lowest of what that
name's entry says (its grant, else its default, else the lake's) and of
what each other name's entry says for it (its grant, else its default).
A grant that is only a `default` is followed too, so a principal is
followed whenever its grants name a type. Each chain of names reads the
grants as they were given, and of the chains that end in a name the one
that took it last continues.

**Where the renames are kept.** A rename happens only in a `migrated`
event. The fold keeps the lake's list of them, each with its position
and type, as one recorded output under a seam name of the store's own,
`sys:renames` (`docs/seams.md`); it is no event, and no door answers it.
A rebuild and an import fold the log and make the list again. An open
reads the list there and looks nowhere else, in any mode: where none is
recorded, no field was renamed. That holds because the fold records the
list at every migration that renames, and every projection a store
reads was made by such a fold: a disk engine refuses a projection
marked with another layout, or with none, until it is rebuilt from the
log (`docs/seams.md`), and the mark is younger than the list. The open
rests on that mark: only a fold older than the list, writing into a
marked projection, could leave a rename with no list, and no such build
knows the mark.

#### A field built from another is read as that field

A derived field hands out what it is computed from: `g = salary * 2`
is the salary, doubled. An embedded vector encodes the text it is
embedded from. So a field is read no further than every field it is
built from, its `depends_on` and its `embed.from`, and theirs in turn.
A source the principal may not read hides the field. A masked source
masks it, unless the field is already hidden. The rule lives in one
place, the field's access (`Sight::field_access` in `lakelands-core`),
so it holds on every door: such a field is absent or masked in `get`,
`find` and a pattern's rows, and in `describe` and the tools.
Filtering, ordering or aggregating over it is refused as not granted,
naming it, as for its source. A search over a vector embedded from a
field the principal may not read still names that source (105).

#### A computed field is read as what it is computed from, listed or not

A field built from another is read no further than what it is built
from. The rule followed `depends_on` and `embed.from`, the lists a
declaration gives, and a declaration need not give the first: a derived
field whose `depends_on` did not name what its expression reads, and a
field computed by a function whose inputs it did not list, were read
whole by a principal the host had kept from the input. Every read door
answered them: `get`, a `where`, an `order_by`, an aggregate, a view's
measure, a pattern's projection and a search.

What a computed field is computed from is not the declaration's to
leave out. So wherever a declaration comes into force in memory, each
computed field's `depends_on` is completed with every field its
expression reads, or with the inputs of the function that computes it.
The rule above then holds as it stands, in every door at once:

- **Nothing logged changes.** The log holds a declaration as it was
  sent; the completion is made when it is read into memory, at the
  declaration and at every open. A lake that holds such a field is
  closed when it is opened by a store that completes; the schema hash
  does not cover `depends_on`, so no hash moves.
- **The completion is made from what was declared, each time.** A
  function declared again with fewer inputs leaves no field held by an
  input it lost.
- **A shape that is not in force is completed before it is judged:** a
  proposed migration's shape and a proposed type, in the listing and in
  a report.
- **`describe` shows the completed list.** It is what the field is read
  by, and a host that reads it sees what its declaration left out.
- **One list.** A field that declared `on_input_change: clear` is
  cleared with every field it is computed from, listed or not. For a
  field computed by a backed function that is a value due again where a
  stale one was kept: a host may see more calls of its backing for such
  a field.
- **An expression is read as it is checked.** A derived expression is
  checked as part of a write, where `previous` and `changed` have
  meaning, and what it reads is told the same way: a field built with
  `previous(b)` or `changed(b)` is built from `b`. Told as a read
  outside a write, such an expression had no reads, and the field was
  read whole by a principal kept from `b`. An expression whose reads
  cannot be told is completed with every other field beside it, of its
  type or of its relation: a field whose reads are unknown is closed,
  never open, so no function that one parse accepts and another refuses
  can open a field again.

An embedding names what it is made from in `embed.from`, which has no
unlisted form; it is untouched.

Why not follow the expression where rights are asked: a field's access
is asked for every field of every record a read answers, a function's
inputs are on another declaration, and the list `declare` checks and
`describe` shows would then say less than the rule that reads it.

#### A field is asked once a pass, and fields that list each other are read no further than each other

In one pass of the walk each field is asked once, however many ways lead
to it: the ways double with every field, so a field asked again on each
of them makes a type of forty derived fields that each read the later
ones answer no read, its declaration holding the lake's writer. Written
fields may list each other as what they are built from, round a cycle,
and **fields that list each other are read no further than each other,
all the way round**: a reader who may not read one of them is not
answered another as read, whichever it asks for first. The walk is made
again, each field asked once a pass, until no answer moves. A field then
has one answer, whichever field is asked for and in whatever order the
type lists them: the widest answers under which every field is read no
further than what it is built from. So of two fields that list each
other, one masked and one its reader writes, the second is written and
not read, being built from a masked field, and the first is hidden,
being built from one its reader may not read.

#### `sensitive` raises an inherited `none` to `masked`, and a `none` by name always hides

`sensitive` raises only an *inherited* `none`, from the type's or the
lake's default for a field the grants do not name, to `masked`, and
only for one who is shown the type (below), so a field nobody thought
about is shown as withheld rather than silently dropped. A `none` given
to the field by name always hides it, and so does a `none` that a field
built from another inherits from its source (above).

### A type is shown only where the grants give something of it, of what it is made of and of what it names

**A type is shown to a principal by what its grants give it: some
access to the type, or to one of its fields. `sensitive` raises a field
to `masked` only for one who is shown the type; it never shows a
type.**

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

- **The one function asks the access before the raise.** Everything
  that asks whether a type is shown asks it, so the description, the
  reads, the doors that write a record, the link doors, the settings'
  judge and what is made of the type follow without a rule of their own.
- **What stays.** Inside a type its principal is shown, a sensitive
  field nobody named is listed as masked, and a filter on it is refused
  as not granted.
- **A field is counted as it is read.** A field built from another is
  read as that field (above), so a grant to read a field, or to see it
  masked, whose source is `none` gives nothing and shows no type. A
  grant to write it leaves it write-only, and that shows the type.

For a host: a principal that is to see that a field is withheld is
granted something of the type, or the field as `masked` by name.

#### A type made of another is shown only where that other type is

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
that relation's target are, a view where its source and every merged
source is. One who is told a type does not exist is told the same of
what is made of it.

- **One function decides it**, the one that decides whether a type is
  described at all. So `describe` and the tools built from it,
  `describe_type`, `find`, `get`, `search`, a pattern's start and each of
  its steps, a set or a trigger on the type, a proposal's listing and
  every refusal that names a type only to one `describe` shows it to
  follow it without a rule of their own. The answer is the one for a
  type that is not declared, never a refusal: a refusal would tell that
  something is there.
- **A grant on its own name does not give it back.** The grants on a
  link type's name and on a view's name stay, follow a rename (above),
  and can take more away; they cannot show what the other type's grant
  withholds.
- **Shown means what it means for any type**: the principal has some
  access to the type or to one of its fields. A principal who reads one
  field of `invoice` is shown its link types and the views over it,
  each under its own grants, and a view still refuses the reader who
  does not read a field its definition probes (105).
- **It runs down a chain**: a view over a view over a link type is shown
  where the type that declares the relation and the relation's target
  are.
- **A relation is named from its target's end only to one shown its
  owner.** A relation and its inverse are declared on the owner, so a
  step from the target by the inverse, or by the relation's own name
  walked in, is answered one who is not shown the owner as a relation
  the target does not have, for a relation carried by links and by a
  reference field alike.
- **A relation is declared to a principal only where it is shown both
  its ends**, and, for a derived relation, may know of every field its
  match pairs, on both sides. A relation to a target that is not
  declared is left out; describing one to a target the principal is not
  shown would tell it the target exists, and the link type's description
  and its `to` field name it. One it is not shown is answered at every
  door as a relation the type does not have, naming only what the caller
  sent: `describe` and `may.link`, a step out, in, either way or by the
  inverse name, a graph and a measure, whose edges leave it out, `link`
  and `unlink` after a retry is answered, a proposal's listing and its
  report, and the hash of the shape shown. A match over a field the
  principal sees masked stays shown: the field is known, and a walk over
  it is refused as not granted, naming it. An author who declares a type
  again from its view keeps such a relation as declared, at its place,
  as it keeps a hidden field (below).
- **A link type is made of its relation's target too.** Made of its
  owner alone, it let a principal kept from the target read the links
  and their `to` keys. Its `to` names the target, so it is followed as
  a view's source is.

For a host: a principal that reads or writes a type's links is granted
something of the type and of its relation's target, and a principal
that reads a view is granted something of each of its sources.

#### A type keyed by a field its principal may not know of is not shown

**A key is shown to whoever is shown the record.** A key made of fields
holds their values. So a type keyed by a field its principal may not
know of is not shown to it: it answers as a type that is not declared,
and so does every type made of it, a link type of its relations and a
view over it. A key field the principal sees masked still gives its
value through the key: the field is known, and only its value is
withheld elsewhere. A key field granted `write_only` is one its
principal may not know of to read, since for reading it is as `none`:
a record key would hand out its value (lakelands#1572). Only the
reading side is withheld: the type is hidden from its reads and shown
to its writes, upsert and retire, since a write names the key and
answers no value of it
(`docs/decisions/092-reading-is-judged-apart-from-writing.md`). A
grouped view's records are keyed by its `group_by`, and an ungrouped
view's one record by no field. A view declares no other `key` (018),
and one that holds another all the same, a host's own type, which
passes no `declare`, is judged by its groups too: no view is hidden for
a field its records do not carry as their key. A grouped view's key is
judged at its sources too: each key field on the view, on its source
and on each merge source by position, since the key holds their values.
It is also the key `describe`, the tools and
a listed declaration name. A view's definition shown to a principal
names no measure of a field it may not know of, as its fields do not. A
measure that reads such a measure by name, directly or through another,
is not known either, as a field derived from one is not: its value would
give the hidden one away. This holds for a principal who may write as
for one who reads, since a view's fields are never written.

For a host: holding a key field at `none` hides the type.

#### A reference to a type not shown is a field not known of, and hides what is keyed by it

To a principal not shown a type, every reference to it is a field it may
not know of, whatever its grant says of the field: read, `masked`,
`write-only` or `sensitive`. While a reference's target stayed shown, a
host that hid a type had to grant `none` on every reference to it, or
the reader was described the target and read its keys through the field.
The reference is left out of `describe`, answered as a field the type
does not have by every probe, read and write, and so are a type keyed by
it (above), a relation carried by it and a valid time over it (105).
One step does this at every place a principal enters: it holds such a
field at `none` in the principal's grants, judged by the types the call
reads, and repeats until nothing more is hidden, since a field so hidden
can hide a type in turn. A field the effective `only` of its type names
is the one exception (109): its value is the one the host wrote into
the grant. The field's access depends on another type, which is the
price of the rule; a host that wants the reference read grants its
target type, the key fields at least masked.

So the made-of rule follows it: a type keyed by such a reference is not
shown, nor a view grouped by one, nor a type keyed by a reference to
either, to a fixed point, and a relation carried by one is not shown. A
host that keeps an agent from a type keeps it, by that alone, from every
type made of it through a key. To show such a type, it grants the target
type, its key fields at least masked; an unkeyed type can be granted
with nothing else.

### A description, a schema hash and a report's id are those of the shape their reader is shown

`describe` holds to the line a probe is held to: a masked field is
described by its shape only — its name, type, access, whether it is
required and sensitive, its authority and what an input change does to
it — never by what it is computed from or what it means, and of a
function, trigger, object set or view that names a field the principal
may not read the definition is not described to it, unless it holds
`schema` (below). A function is shown to a principal only where
`describe` lists it (every type that computes a field by it is shown to
the principal, which reads that field and each input, or it holds
`schema`). A value type is listed, with its version, only to a principal
shown a field that uses it (a type's field, a relation's link field, a
verb's payload or a function's input, each where `describe` shows it, a
masked one too, whose shape names the value type) or holding `schema`:
its `one_of` and `constraint` say what those fields' values may be. One
that no field uses is listed to holders of `schema` alone
(lakelands#1088).

#### A field its reader may not read is described without its annotations

A field the principal may not read is described without its annotations,
as without its description and label. This holds for a type's own field
and for a relation's link field alike.

`describe` answers a type's relations with their link fields. A link
field the principal may not read is described by its shape alone,
without its derivation, meaning or annotations. A type's and a
relation's own annotations are shown to whoever the type is described
to, as their descriptions are.

An annotation grants nothing and restricts nothing. What a principal
may read is decided by its grants alone. A host that marks a field with
a policy of its own enforces that policy itself.

#### A field's declaration is shown by one list, wherever it is answered

`describe` showed a field its reader may know of and not read by its
shape alone, and a migration's report, a proposed migration's `to` and a
proposed declaration's types did not: a reader with a field masked was
handed its label, its description, its annotations, whether it is
searchable and the expression it is computed by, in the listing of a
proposal and in a report's `restated`.

**One list, in the contract.** The members a field shows only to one
who reads it (`derived`, `function`, `depends_on`, `searchable`,
`description`, `label`, `embed`, `annotations`) are named once beside
the field's declaration, with the step that clears them. `describe`, a
relation's link fields, a report's `restated` and both listings go by
it, so a member added to the list is hidden everywhere at once.

**In both shapes.** A field's access is built from what it is computed
from, and a migration may change that with no rename: a derived field
read from `secret` and in `to` from `name`. So an entry of a member on
the list is shown only where its reader reads the field in `to` and in
the type as it stands, under the name it had; its shape (`sensitive`
among it) is shown where the reader may know of the field.

**An author's view is not a listing.** The view an author declares or
migrates from holds every field it may know of as declared, so that a
field it does not read stays as it is; the list clears a declaration
only where one is handed out. An author with a field masked is
therefore answered of its own migration the shape and `more`, not the
members it never saw.

#### A view's definition is shown to those its read answers

`describe` lists a view's type to a principal who may not read all the
definition reads, with the definition withheld. A proposed bundle's
listing leaves such a view out and says `more`: a declaration without
its definition would read as a stored type
(`a_proposed_shape_names_only_what_its_reader_may_know_of`).

`describe` and the read of a view each held a rule for the fields a view
reads, and they differed twice. `describe` counted what a measure
aggregates; the read probed every name in a measure that the source
declares. And the read went on into a source that is itself a view,
where `describe` stopped at the view's own sources. So a reader could be
shown a definition and refused its read, with nothing in the definition
to say why.

So there is one rule. A view reads of each source its grouping, what
its `where` reads and what its measures aggregate, and a source that is
a view is read through, to what that view reads. `describe` shows a
definition by it, a read of the view probes by it, and `declare` and the
migration preflight ask the same of a view's own sources.

A name a measure bears outside an aggregate is no field of the source.
It is read from the group, an earlier measure or a grouping field,
whatever the source holds under that name (027, 028). A read's probe of
such a name guards no value, and it tells: of two views alike but for a
measure's name, the one named as a field of the source was refused to a
reader kept from that field and the other answered, so the reader
learned that the field is declared. The read does not probe it. A view
that names an earlier measure is shown and answered the same, byte for
byte, whether or not its source holds a field of that name.

So a definition is shown to a reader exactly where the view's read
answers it. A holder of `schema` is the exception: it is shown every
definition, and is refused the read it may not make, on the field, since
showing a definition is not reading what it reads. An object set is
shown and read the same way: its definition to a holder of `schema`, its
records to one who reads what its `where` reads.

#### A schema hash is that of the shape its reader is shown

A reader rebuilds the declaration it is described (each field's name,
type and `required`, which `describe` gives even of a masked one, the
key, the relations and the view) and hashes it with the public
`schema_hash`. Were it answered the stored hash, the two would differ
exactly when the type has a field it may not know of, and hashing
candidate fields would name it.

**The hash is the shown shape's.** Wherever a schema hash is answered,
it is that of the declaration as its reader is shown it: without the
fields and link fields it may not know of, and without a view it is not
described. A masked or write-only field hashes as declared, since the
hash covers only what its shape alone keeps. Where nothing is left out,
the answer is the stored hash, byte for byte, so the owner and the host
see no change. This holds in `describe`, as of a position too, and in a
migration's report: its preview, its refusal and its proposal's
listing. In a report, `from` is the shape of the type the report was
made from, so a proposal made before the type changed is not shown as
current, and `to` is judged with the grants held through the
migration's own renames, as its listed `to` is. A listed proposal is
judged with its reader's grants followed to the proposal's position
(above), so a field renamed since and hidden under its new name is in
neither hash. `more` is decided before the hashes are set, so it says
nothing the hash would.

#### A report's id is made over what its reader is shown

A report's id names the report and the position it was read at, the one
039 names: the log's last event that is neither a `lease_taken` nor a
`checkpoint`. It is computed over the report as its reader is shown it,
so the reader can compute it from what it holds and learns nothing from
it; the reply's `position` is that position unless a lease or a seal
stands after it. `migrate_apply` compares the caller's own id, and
refuses it once a write since has reached what the report names, while a
write elsewhere leaves it good (099); an accept compares no id, and a
listed proposal's id is made at the position 039 names before the
proposal (lakelands#1476).

#### The stored report, the log and the feed hold the whole hash

The stored report, the accept's comparison (a type changed since the
proposal, in a hidden field too, refuses it), the log's `declared` and
`migrated` events and the feed hold the whole hash. One version of a
type may therefore have several hashes, one per shown shape, and a hash
compares only between readers shown the same shape and across one
reader's answers over time; the version moves on every change, a hidden
one included, which a description already does without the hash.

#### The rest of a type's description is computed over the same shown shape

The rest of a type's description is computed over the same shown shape.
Whether the type is searchable and whether it is sourced are said of the
fields its reader is described: a field it sees masked is described
without `searchable` and is searchable to it no more, and one it may not
know of says nothing. A relation is described only to a reader shown
both its ends, the field that carries it where it is carried by a
reference field, and, for a derived relation, every field its match
pairs (above).

A type described as of a position is told the names it bore after it
(`035 § A renamed relation keeps its edges`) only where its reader,
its grants named at the head, is shown it at the head under its last
name too. The position judges whether the type is described at all;
the head judges whether its new name is told. A reader shown the old
link type as of before by a permission on the old name alone, which
gives nothing under the new one, is not told the new name: it would
learn the name of a type it is not shown. A reader not shown the type
at the position is described nothing of it, rename included.

## Why

006 states why the store decides a call on grants rather than roles, why
the check sits in the store, and why masking an answer does not protect a
value the question already revealed (`006 § Why`). The reasons particular
to these rules are stated with them, above.

## What it is not

006 states what no rule of grants decides: it is not an off switch, not
authentication, and not partial success (`006 § What it is not`).

## How it came to be

- These rules were stated in 006 and changed there from its first version;
  lakelands#1768 moved them here word for word. Their history, oldest
  first, is that record's (`006 § How it came to be`).
