# 006 — Grants, not roles: what a principal is shown and told

## What is decided

This record holds what a principal is shown and told: what a read, a
probe, a write, a call, a declaration, a listing and an accept answer
it, and what a refusal may name. It holds several rules and is as long
as they need. 024 and 051 point here: each maps the sections it held to
the headings below. 013 and 027 keep their own topics, verbs and shape
changes, and point here for what moved.

### A call carries one principal with grants, and the store checks every read and write

The store knows principals and grants, never roles. Every call carries
one `Principal` (kind, id, lake, grants), and the grants say, per field,
what that principal may read and write. The store enforces them on every
read and every write: a field the principal may not read is masked or
absent in the answer, and in the schemas the generated tools expose; a
write carrying a field the principal may not write is refused whole.

Turning a host's identities, roles, teams or policies into grants is the
host's job, done in its `PrincipalResolver`. Lakelands' own default
resolves an API key or a static configuration entry to one principal.

What a handle keeps of the lake's shape between reads holds nothing of
a reader (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`):
what is shared is what the lake declares, before any rule of who is
shown what. A principal's grants, its renames followed and its `only`
are applied per read.

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
other field is writable too.

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
A write is followed before it waits for the writer, and a migration can
rename a field while it waits. Where the list has grown when the write
takes the writer, it is refused to be sent again; an ingest is followed
under the writer itself.

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
place, the field's access (`lakelands_core::rights::field_access`), so
it holds on every door: such a field is absent or masked in `get`,
`find` and a pattern's rows, and in `describe` and the tools.
Filtering, ordering or aggregating over it is refused as not granted,
naming it, as for its source. A search over a vector embedded from a
field the principal may not read still names that source (below).

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
  does not read a field its definition probes (below).
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
view over it. A key field the principal sees masked or may only write
still gives its value through the key: the field is known, and only its
value is withheld elsewhere. A grouped view's records are keyed by its
`group_by`, and an ungrouped view's one record by no field. A view
declares no other `key` (018), and one that holds another all the same,
a host's own type, which passes no `declare`, is judged by its groups
too: no view is hidden for a field its records do not carry as their
key. A grouped view's key is judged at its sources too: each key field
on the view, on its source and on each merge source by position, since
the key holds their values. It is also the key `describe`, the tools and
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
it (above), a relation carried by it and a valid time over it (below).
One step does this at every place a principal enters: it holds such a
field at `none` in the principal's grants, judged by the types the call
reads, and repeats until nothing more is hidden, since a field so hidden
can hide a type in turn. A field the effective `only` of its type names
is the one exception (below): its value is the one the host wrote into
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

### What is hidden answers at every door as what is not declared, and what is masked is named and refused as not granted

The two grants that withhold a value say different things. `masked`
tells the principal that the field exists and withholds its value:
the field is listed as masked, described by its shape, and a read that
probes it or a write that carries it is refused as not granted, naming
it. `none` says the field does not exist for that principal. It is
absent from answers, from the masked list and from metadata, from
`describe` and from every generated tool. A probe or a write that names
it gets the answer for a field the type does not declare: the same
kind, the same sentence, the same field named. Any other answer would
let a single refusal reveal what every answer hides.

#### One sight answers what a principal is shown, at every door

Whether a type, a relation or a declaration is there for a principal was
asked at every door by the door's own means: a read described the type,
a write asked the type's rule, a listing built the whole description, and
the call door and `describe` each held a rule of their own for a verb.
Each rights finding of the rounds before closed one door, and its
verifier then found a sibling that answered the same question another
way. So the question has one place: a sight of the door's principal over
the declarations in force at its position, which every door asks: `get`,
`find`, `search`, a pattern and each of its hops, `describe`,
`describe_type` and the tools made of them, `upsert`, `retire`, `link`,
`unlink`, `call`, `propose`, `accept`, a listing, a migration's
preflight and a refusal that would name a declaration. The sight is
made of the principal the door is called with: its grants are held
through the renames, and its references to types it is not shown are
held at `none`. The listing of proposals, and the accept and the reject
that go by it, make more than one: of the lake in force, of the lake as
an entry would leave it, and of the lake as the entry was proposed
against, each of the reader as its call named it, its grants followed
there (below). Following comes first, since a grant held through a
rename may hide what the grants as given do not. A write judges under
its lock with the principal its call was made with. Where the types
moved while it waited, its sight is made a second time, over the head's
types, and the write is refused to be sent again where the head hides
other references from it than its call was shown; one that waited while
types moved that hide nothing more from it is applied.

What is shown does not turn on a read's valid time. A read at `valid_at`
filters records by the type's valid-time fields, so the read that
filters probes them, and is refused where its reader may not read one
(below). A listing of proposals filters no record by them: a proposed
write to such a type is listed at a valid time as without one, and the
accept that follows judges it as the listing did.

The doors are held together by one row: for each of a cast of principals
and each type of a lake, a type `describe` names is at no door a name
that is not declared, and one it does not name is at every door exactly
that: at a read, a write, `link` and `unlink`, an ingest, a call, a
proposal and a migration's preflight. The accept and the reject are
held to the listing by a second row, a grid of proposed entries and
reviewers (below).

#### A database's error quotes no row

An engine's error reaches the caller as `unavailable`, and the network
doors answer it whole. PostgreSQL adds a `DETAIL` to many errors, and
for a failed constraint it quotes the failing row: a record's key and
its values, which the caller may not be shown. So the PostgreSQL engine
answers only the severity, the SQLSTATE and the server's primary
message, which names a table, a constraint or a column and no value;
`DETAIL`, `HINT`, `WHERE` and the statement position stay out of the
answer. SQLite's own messages name the columns of a failed constraint,
or the schema's own check, and never a row's values, so the SQLite
engine answers a failure SQLite reports by its extended code and that
message. Its driver's own errors do quote: a statement that does not
parse is answered with its text, and a stored value that does not fit
its type with the value. Of any such error the SQLite engine answers
only its kind.

### A shown type's records may be found and got, and a probe needs the right to read what it probes

#### A principal that is shown a type may find and get its records

**A principal that is shown a type may find and get its records, and
`may.find` and `may.get` say so of every type `describe` answers.**

`describe` said `may.find` and `may.get` only of a type its principal
reads a field of. The read doors never asked that: a principal shown
`invoice` with every field masked, with one field masked by name, or
with a field it may only write, was answered every invoice by `get` and
`find`, counted them and walked from them, while the description said
it may not, and the tools built from the description offered it neither
read nor named the type in the pattern, search and subscribe tools.

- **A record is its key, its version and what the grants show of each
  field**: the value of one that is read, the name of one that is masked,
  in the declaration's order, and nothing of one that is hidden or only
  written. One who reads no field is still answered the record; that is
  how it sees what is withheld, and what a writer writes against.
- **The probe is refused.** A filter, an order, an aggregate
  or a search over a field the principal does not read is not granted
  (below), so `may.find` promises the door, not every question.
- **A step walks a relation by its owner.** A relation is declared on
  the type the principal is shown, so a step over it is answered the
  other end's key also where the principal's grants keep it from the
  relation's link type; it is then shown no link field.

For a host: a principal that is not to list a type's records is granted
nothing of the type.

#### A read that filters on a field, orders by it, aggregates over it or searches it needs the right to read it

A read that filters on a field, orders by it, aggregates over it or
searches it is refused when the principal may not read that field —
also when the field would have been masked in the answer. The refusal
names the field. Where the principal may not know of the field at all (its
access is `none`), the refusal is the one for a field the type does not
declare (above).

#### A read at a valid time probes the valid-time fields its reader is shown

A read at `valid_at` filters on its type's `valid_from` and `valid_to`,
so it probes them as a filter on them would: `find`, `get`, `search`, a
pattern's start and every type a walk reaches. It filters by the bounds
the reader is shown and probes only those: with both hidden it answers
as for a type with no valid time, with one hidden as for a type declared
with the other alone, word for word. A refusal, or a filter by the
hidden bound, would tell the reader that a valid time it is not shown
exists.

A walk keeps each record it meets by its own type's valid time, so it
probes every type it can meet, not only the one a step is judged to
reach. That is the far end of each relation the step walks, the set its
relation's probe and its read ask for, a hop at a time from where the
step stands to its range's last hop: every type its relation reaches on
the way where it names no `type`, and with a `type` that type and those
its walk passes through to it. A graph from depth 0, a path and
`components` walk a step's relation from every record they meet, so for
them it is each type the relation reaches wherever it is declared,
within the step's depth or not, or the step's `type` alone where it
names one. A step of no hop meets no record: it probes the type it
names, and no other. The set is read from the declarations the reader is
shown, before a record is read: a refusal that waited for a record of
the type would say that one was reached.

A view read at `valid_at` is computed from its sources' records as they
are valid then, so the computation probes each source's valid-time
fields too, those its reader may know of, a merged source's as the
first, and as deep as the view goes: a view over a view is computed from
the inner one, whose computation probes its own source. The probe stands
where the computation reads a source, beside the view's other probes, so
every door that computes the view makes it: `find`, `get`, a pattern's
start, and a walk where it computes the view it arrives at. What reads
no source at a valid time probes none: the kept records of a view, and
`describe`, which shows a view alike with a valid time and without. The
probe is the view's own: it names the field to a holder of `schema` and
nothing of the source to another reader.

#### A search probes what it searches, in every mode

A search probes the searchable fields of its type it searches, in every
mode, and the vector field it names. One that names no fields searches
the searchable fields its principal reads: a field it sees masked or may
not know of is not searched, so it neither refuses nor ranks the search,
and where every searchable field is hidden from it the search answers no
hits. A vector embedded from text encodes that text, so a vector or
hybrid search also probes every field its vector is embedded from, even
when the principal may read the vector itself. A hybrid search that one
of these refuses is refused whole, never answered from its other half. A
hybrid search the store embeds the text of is judged so before its
field's embedding model is compared with the bound backend's and before
the text is sent: the model is said only to a caller who may make the
search, and a search refused calls no backend.

A lexical or hybrid search may name the searchable fields it matches in
(`fields`). It then probes those fields instead of the searchable ones
it reads, so it may match in a field its reader may not read only by
naming it, and is then refused. Only the text half narrows: the vector of
a hybrid search still encodes every field it is embedded from, and is
probed as above. A named field the principal may not read is refused as
not granted before its searchability is judged, so the refusal never
tells whether a field it may not read is searchable; a named field that
is not declared or not searchable is invalid.

#### A view's own probe names nothing

A read of a view probes what the view's definition reads: its `where`,
its grouping and what its measures aggregate, in its source and in each
merged one. A reader who may not read one of those fields is refused, as
for any probe. But the field is one the view named, not the reader, and
the answer for a field the reader names itself ("`invoice` has no field
`salary`", or not granted on a masked field) would tell it what the
definition reads. `describe` withholds that definition from such a
reader (below). So the read is refused as **not granted, on no field**,
and its text names neither the field nor the source: the view is one
this reader may not read. `find`, `get`, and a pattern that starts on
the view or steps onto it answer alike.

A principal with the `schema` grant is shown every definition, so the
field's name tells it nothing new, and it is the one who can act on it:
it keeps the answer that names the field.

#### A named set is read by its filter's right

`in_set('open')` reads what `open`'s filter reads, and what the sets
that filter names read. So every rule that asks what a condition reads
asks it of the filters in the names' place: a read probes those fields;
`describe` lists a set or a trigger to one who reads them all; a write's
answer names a set joined or left, and a trigger fired, to one who reads
them all; an author declares a condition naming a set only where it may
read them.

To a reader kept from a field, a set whose filter reads it is a set that
is not declared, by its name as in `describe`: the refusal is the one a
name no set has gets, and names nothing of the field.

A read naming a set of a type its reader is not shown, where it reads
every field the set's filter reads, was told that the set is of that
type: the set is one that is not declared, as the type is at every
door (above), and so in a trigger's `when`; to declare a set under its
name is not granted (below). A holder of `schema` is answered the same:
it is shown the definition only of a set whose type it is shown.

#### A relation carried by a field is walked by its reader only

A relation carried by a reference field (`via`) answers that field's
value with every edge: a walk out of a record reaches the record the
field names. So the relation is read as its field is. To a reader who
may not know of the field the relation is not shown: `describe_type`
does not list it, and a walk over it, out, in or as a graph, is answered
as over a relation the type does not declare. To a reader who sees the
field masked the relation is shown, and a walk over it is refused as not
granted, naming the field, as a walk over a derived relation's masked
pair is. Every hop is probed so, a derived relation's matched fields
too, around the type it reached, not only a step's first: a walk of two
hops, a graph or `components` that meets such a relation or pair past
its first hop is refused alike. A walk reaches no edge of a relation its
reader is not shown, whatever step asked for it, so a step over another
type's relation of the same name walks only that one. An author with
`schema` who may not know of the field sends a type whose target it is
shown, so a relation it sends under the hidden relation's name is
refused as one declared twice, in the words any type declaring a
relation twice gets, and the type in force does not change (below).

#### A step with a `type` probes what it keeps

A step with a `type` walks only the relations that can lead to a record
it keeps, and what it does not walk it neither probes nor reads. Probed
over every relation of its name around the type it stands on, whatever
its `type`, a step in over `of` with `type: bill`, where a bill and a
quote are each `of` a customer, was refused to a reader masked from the
quote's reference field alone, though no quote is in its answer.
A neighbourhood and a path keep a record at every hop, so their step
walks a relation only towards its `type`. A chain's step, in rows, an
aggregate and a chain's graph, walks its whole range before it keeps, and
a record of another type may lie on the way: it walks a relation towards
its `type` and towards each type its `type` is reached from within the
step's range, so a relation it walks through is probed too. Each
step is read by its own rule, at every hop, though another step walks
the same relation the same way. The probe and the read are one set: the
engine's edges of a relation outside it are dropped before the walk sees
them and before a link record is read for them, so the answer is, to
the byte, the one a lake without those edges gives, and a refusal names
only a field the walk would have read. A step with no `type` is probed
over every relation of its name around the type it stands on.

A step whose `type` it does not reach walks nothing, so it is told what
it reaches, the types its reader is shown, also where a relation of its
name is masked.

#### A pattern's probes ask whether a type is shown, too

An order, a measure, a grouping or a projected field over a path on a
type the principal is not shown is refused as a path no bound name
starts, word for word as one on a type that is not declared (018). A
step over a derived relation probes the matched fields of the relations
it walks and of no other: its own out of the type it leaves, and one
into that type whose owner the principal is shown. A relation a hidden
type derives under the same name reaches no record the principal is
shown and is not probed, so no refusal names it.

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
`schema`).

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

A report's id names the report and the head it was made at, and it is
computed over the report as its reader is shown it, so the reader can
compute it from what it holds and learns nothing from it.
`migrate_apply` compares the caller's own id; an accept compares no id,
and a listed proposal's id is made at the head before the proposal.

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

### A write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover

A refusal is an answer too. A write that would break a rule about the
lake's data, refused before the writer's grants are checked, tells a
principal without those grants what the data holds: that a record
already links somewhere, and to which key; that a type exists and has a
field. So the line drawn for reads holds for writes:

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
  `retire` orders it.

The rule says which refusal comes first, not what a write checks.

#### A setting the store reads is judged by what its writer is shown

A setting the store reads is judged the same way. `sort_index.<type>`,
`vector_index.<type>` and `view_kept.<type>` name a type, its fields
and, for a view, what its measures read, and the judge's sentences say
whether each is declared, of which kind, and why a view cannot be kept.
An applied write tells as much as a sentence does, and it acts: an index
is built, a view is kept. So the judge comes after the writer's grant on
`sys.setting`, and it is handed the type as `describe_type` answers it
to the writer, the one function that already decides what a principal is
shown:

- A type that is not shown is a name that is not declared, under every
  value: `null` and `false` change nothing, and would otherwise tell a
  declared type from none.
- A field the description does not name is one the type does not
  declare. A field it names masked or write-only is judged by its kind:
  the description says the kind already, and an index changes no answer.
- A view shown without its definition cannot be kept, and the refusal
  says no more: the reason names a measure, a call and a field of the
  source, and is said to one shown the view as declared, as `kept` says
  it.
- The judge reads the value the write leaves, with no exception for a
  name that stood before. A principal extends or patches a setting only
  while it is shown everything the setting names; otherwise the setting
  is left to one who sees more. Retiring a setting is not judged.

The rule was weighed against letting a standing name pass unjudged: that
keeps one write, and costs a second path through the judge and an answer
that depends on who wrote the setting first.

#### Every door that writes a record asks first whether its writer is shown the type

A principal shown nothing of a type was told by its writes what
`describe` kept from it: an `upsert` was refused field by field (``
`vault` has no field `code` ``), a `retire` and a verb's call were
refused as not granted, a write to a view was told it is a view, and
each of these is another answer than the one a name that is not declared
gets. So every door that writes a record asks whether its writer is
shown the type, the question the link doors ask, before anything else is
judged:

- **`upsert`, `retire`, `propose` of either and `ingest`** answer one
  not shown the type as for a type that is not declared, before the
  patch, the key's form, the view's refusal, the grants and the version
  are looked at. `ingest` asks its own grant first: one without it is
  refused whatever it names.
- **A verb on a type its caller is not shown is a verb that is not
  declared**, for `call` and for `propose`, before its scope is judged
  in a sentence that names the type. A caller that is shown the type is
  told what it may not do there: a verb it may not call is refused as
  not granted.
- **A retry is answered first**, as at the link doors: a write made
  under an idempotency key is answered that write to the principal that
  made it, whatever it is shown since, as every retry is before the
  grants are asked. It tells the caller its own write.
- **It is asked of the type the caller names.** `link` and `unlink`
  reach a link type by its owner's name, so they ask whether the caller
  is shown the owner before they name a relation, and answer one who is
  not as for a type that is not there, whether the type has the relation
  or not; the link record they write is then judged by its grants, so a
  caller shown the owner and kept from the link type by its own grant is
  refused by that grant. A verb is asked of its type once, at the call;
  the record it writes is not asked again.
- **A proposal is asked of its proposer when it is made.** Its accept
  asks its reviewer, as the listing does (below), and writes as the
  proposer; aging writes as the store.

The question is the two tests that decide whether a type is described,
some access to the type or to one of its fields and the same of every
type it is made of, as one function that the description calls first.
Asking the description itself was measured and refused: it copies the
lake's declarations, 209 µs a write in a lake of two hundred types
against a write of 75 µs, where the two tests take nanoseconds.

#### A write's answer names a reaction to one who reads what its condition reads

`fired`, `joined` and `left` name a trigger or an object set
only where the one answered is listed it by `describe` and reads every
field its condition names. Being shown a definition is not reading what
it reads. So a condition another principal declared, or one in force
from before, tells its reader nothing by name.

### A verb is callable and described only to one who may write what it sets and read what it reads

A call is held to its caller's right to write each field the verb sets.
A verb that reaches outside the lake, called on a record or on its type
(013), takes its type's grant: write access to `on`.

A verb judged by the fields it writes alone tells more than its caller
may read. Its `requires` and the values it sets read the record too, and
a caller who may not read a field they read learned it all the same: a
verb that requires `secret = 'x'` runs on one record and is refused
`requires_unmet` on another, and a verb that sets `copy := secret`
copies the value to a field its caller reads. So a verb is callable, and
described, only to a caller who may read every field its `requires` and
its set values read, each judged as a filter on that field is. A field
read across a reference counts with every reference on the way and the
types they name: a verb whose path reads `customer.discount_days` is its
caller's only where the caller may read `customer`, is shown the
customer's type and may read `discount_days` there
(`docs/decisions/055-a-path-steps-through-a-reference.md`).
`payload.<name>` reads the call's payload, which is the caller's own. To
any other caller the verb answers `not_granted`, as one whose fields it
may not write does, before the record is read, and `describe` names it
neither on its type nor among the lake's verbs. This holds for a direct
call, a call its risk proposes and a call proposed through `propose`. An
accept judges the record again, so its reviewer is held to the same
rule. A call is proposed only where its `requires` held, so a call
proposal tells what the verb reads as well. Its `why`, when the store
wrote it, names the verb, which `describe` withholds from that reader,
as it withholds a type the reader is not shown. So a call proposal of
such a verb is not there to a reader who may not read what the verb
reads, as a proposal on a hidden type is not (below). `proposals` leaves
it out, so its count does not tell either. `accept` and `reject` answer
as for an id no proposal holds, before anything else of the proposal is
judged. So a reviewer is not told that the verb was declared again
since, which would name it. The verb is judged as the lake declares it
when the proposal is read or decided, and as it was declared where the
call was proposed: an owner who declared the verb again without its
`requires` would otherwise bring back, key and `why`, every call
proposal the rule had hidden. A field derived from one the caller may
not read, a write-only field and a reference to a type the caller is not
shown are judged the same way, since a filter on them is. A verb that
reads no such field is judged by the fields it writes alone.

A type its principal is not shown is not named by what is declared
beside it either. A verb on such a type is not described and no tool
is made of it, even where the principal could write every field the
verb sets: `call` and `propose` answer it as a verb that is not
declared (above), and `describe` agrees with them.

### A declaration or a migration is its author's view

A field that does not exist for a principal on every read door is not
reached by that principal's change of shape either: its migration and
its redeclaration are read as its view of the type, and the hidden
field is carried over as declared. The preflight's report names no such
field to it, and a proposer reads its own proposal without it.

A principal writes a type's new shape from what `describe` answers it,
and `describe` leaves out a field the principal may not know of. Read
literally, such a shape removes that field: a principal who was never
told of `salary` would propose it away, and the lake could not tell that
from a removal it meant. So a migration's `to`, and a type declared
again, is read as its author's view of the type:
- a field hidden from the author (`rights::field_access` answering
  `none`, by its grant or through a field it is built from) is put back
  as declared, at its declared place, and so is such a link field of
  each relation the shape keeps;
- the report is made on the completed shape: `dropped` lists what the
  author saw and removed, and never a field the author may not know of;
  the report id names the completed migration;
- `migrate`, `migrate_apply`, `declare`, `apply` and `propose` complete
  the shape from their caller's rights. A proposal logs the completed
  shape, so `accept` completes nothing, and a reviewer who may not see
  the field cannot remove it by accepting either.

No refusal on the way names a field its author may not know of. A shape
that names one declares it twice, which is how a taken name is refused.
A rename from one is a rename from no field. A shape that removes a
field a hidden one is built from is refused on the field it removes,
saying only that another part of the type reads it. A relation its
author is not shown is kept the same way: put back as declared, a field
its `via` or its match reads refused on that field alone, a rename from
it a rename from no relation, and a shape that sends a relation of its
name declares that relation twice, refused in the words any type that
declares a relation twice is. Each is said where the same fault of a
name the lake does not have is said: the migration as its author sent it
is judged against the type as its author is shown it, by the rule and at
the step that judge the type's own names, so a second fault in the
migration comes first or after alike, and the order tells nothing. A
reference field to a type its author is not shown is a field it may not
know of (above), kept the same way; one the author sends names a type it
is not shown, and is refused as one that is not declared, so naming its
target in force answers as naming any other hidden type or no type, and
the author learns nothing of the target. A masked or a write-only field
is shown to the principal, and stays its to remove.

What a type's shape names stands as declared only where it stands. A
valid time left out where the one in force names a field outside the
author's view, which `describe` does not name to it (below), stays as
declared: it is left out of the author check and checked whole at apply,
so no new member can name that field through it, and it is put back with
the hidden fields. One the author sends, the one in force restated too,
is judged by its name first, as on a type with no valid time: a name no
field has, or a field the author may not know of, answers word for word
as there. A field the author may name is refused at the type: the bound
is taken, as a name a hidden field holds is, and it stands. A key and a
relation's `via` never name such a field: a type keyed by one is not
shown (above), nor a relation carried by one, nor is a reference to a
type the author is not shown known to it. A key or a `via` its author
gives anew is read against the author's view, where such a field is no
field. A hidden field's name is therefore answered word for word as a
name no field has, by `declare` and by `migrate`, and the type in force
is not changed.

#### A declaration reads only what its author may read

A principal with the `schema` grant that was kept from a field could
still read it, record by record. It declared an object set or a trigger
whose condition named the field, and the answer of a write named the
set a record joined and the trigger that fired. It needed no declaration
of its own where another's condition read the field: `describe` shows
one who holds `schema` every definition, and a write's answer named
every reaction `describe` lists to its caller. The same declaration also
told whether a name was a field's: one over a field the type has was
taken, one over a name it has not was refused.

**A declaration reads only what its author may read.** Where a
declaration is its author's, at `declare`, `apply`, `add_field`, a
migration and a proposal of either:

- **A field its author may not know of answers as one the type does not
  have.** The set is checked, as sent, against the ontology as its
  author is shown it, by the checks every declaration passes. So the
  answer is the one a name the type never had gets, word for word, and
  it holds for whatever reads a field: a set's `where`, a trigger's
  `when`, a verb's `requires`, what a verb sets a field from and the
  field it sets, a derived field, a field computed by a function, what
  an embedding is made from, a view's `where`, grouping and measures,
  and the pairs a derived relation matches. A reader added later is held
  with them, since it is the existing check that refuses.
- **A field its author knows of and may not read is refused as not
  granted on it**, wherever the declaration reads its value. A verb may
  still set such a field: the declaration grants nothing (above).
- **What a type's shape names stands as declared only where it stands**
  (above). A valid time and a relation's `via` are named by the shape,
  not read by it. A key field is different: an author who may not know
  of one is not shown the type (above), so its declaration of the type
  is not granted and a migration of it finds no type.
- **Only what its author adds or changes is its author's.** A type that
  holds another's derived field over a field its author sees masked is
  declared again as it stands; a field of the author's own over the same
  field is refused.
- **`add_field` adds to its author's view of the type.** Built from
  the whole type, the new shape let a field derived from a hidden one
  in, and an author kept from any field could add none.

**Not at the accept.** An accepter did not write the declaration, and a
refusal there would name a field to one who may not know of it. The
author is held at the proposal; a proposal made before a lake ran this
is accepted as it was proposed.

A declaration in force that reads a field its declarer may not read
stays in force. It is held when it is declared again, and until then a
write's answer names it only to one who reads what its condition reads
(above).

#### A set named in another's filter is judged by the sets its author is shown

A set named in another's filter is judged the same way. A proposer
without `schema` that proposed a set, or a trigger whose `when` names
one, was checked against every set of the lake, so `in_set('open')` over
a set it is not shown was refused as reading a field the type does not
have, the one that set's filter reads, where a name no set bears is
refused as undeclared: the refusal told that the set is declared and
what it reads. So its proposal is checked against the sets it is shown,
and the two names answer alike. A verb's `requires` and a view's `where`
take no set at all. A holder of `schema` is shown the definition, and is
still refused on the field it may not read.

To propose a set under the name of one its proposer is not shown was,
for another type, refused by the set's own rule, which names the type
it holds, and for the same type proposed, as a set declared again; it
is not granted, for either type, as to declare a type one is not shown
is (below).

#### A type its author is not shown is a type that is not declared

A principal with the `schema` grant that is not shown a type could
still learn of it, and change it. `add_field` told which of its names
were fields. A migration of it with no field was applied. A trigger, a
verb and an object set on it were declared, where the same on a name no
type has were refused. Seven of these doors wrote an event.

To its author such a type is one that is not declared:

- **`add_field`, a migration, its preview, its apply and its proposal**
  answer that there is no such type, as they do for a name no type has.
- **A trigger, a verb and an object set on it,** declared, applied in a
  set or proposed in a bundle, are refused as on a type that is not
  declared, in the same words. The set is checked against the ontology
  as its author is shown it, which does not hold the type (above).
- **To declare the type itself is not granted,** and so is to propose
  it. This is the one door that cannot hide that the name is taken: for
  a free name it creates the type. The refusal names no field and tells
  nothing else, and nothing is written. An object set its proposer is
  not shown is the same: to propose a set under its name, of any type,
  is not granted, where the refusal a set's own rule gives would name
  the type it holds. A trigger and a verb are the same, and for them it
  is more than a name: each is known by its name alone, so one declared
  under the name of one its author is not shown took its place, and what
  fired or could be called on a type was gone by the hand of one who was
  never shown it. To declare, to apply or to propose it is not granted.
  Not shown is what a proposal's listing would not show that author of
  the one in force: a trigger or a verb on a type it is not shown, and,
  to an author without `schema`, one whose `when`, `requires` or values
  read what it may not read, or that sets a field it may not know of. A
  verb is judged by what it names, not by who may call it. A function is
  the same to a proposer that is not shown it, one that computes a field
  of a type that proposer is not shown, or a field or from an input it
  may not read: accepted, its new body would compute that field again. A
  function that computes no field is shown to no one without `schema`
  and is not such a one: declared again it takes nothing from anyone and
  computes nothing again, so its name answers as a free one does. A
  holder of `schema` is shown every function, so this line does not keep
  it from one that computes a field of a type it is not shown. An author
  that is shown the one in force moves it to another type, or gives it a
  new body, by declaring it again.
- **A tool's name is taken the same way.** A type, a verb and a relation
  carried by links each give tools their names, and the lake refuses a
  declaration whose tool would bear the name of another's, naming both.
  Where the other is a type, a relation or a verb its author is not
  shown, that refusal told it what it was kept from, whichever of the
  two was declared. The declaration is not granted, naming nothing,
  before the set is checked: it is refused where the lake with the set
  in it has a problem with its tools' names that the lake as its author
  is shown it has not. A type declared again keeps, for this, the
  relations its author is not shown, as its view is completed with them,
  and a set is read with the link types it brings, as its check reads
  it. A migration's shape passes the same line, at its preview and at
  its proposal. Between what its author is shown a collision is refused,
  naming both, and a relation under the name of one its author is not
  shown is one declared twice, as a field is.
- **No door leaves a lake with a problem among its tools' names.** The
  line above rests on it: a problem the set brings then has a side
  among the set's own names, and the other is looked for among the
  names of what its author is not shown before the two lakes are
  compared, so a declaration that meets none there copies nothing. A
  declaration is checked with the names of the link types it brings,
  alone as in a set, and a migration that reshapes is read for its
  tools' names as one that does not.
  A writer's open is such a door (lakelands#893): the system types it
  declares, a host vocabulary's among them, are judged beside the lake
  and beside each other before anything is appended, and an open that
  would leave a problem is refused as `tool_names`, naming both sides
  to the host, whose vocabulary it is (`docs/seams.md`,
  `SystemVocabulary`). A type the lake holds already is not judged
  again. An import carries a log that no door of this lake judged: there
  the rule holds by the doors of the lake that wrote it, and a log
  written with such a problem is taken whole and refuses its next
  declaration as its source does.

The test comes before the author's view is completed and before
anything is appended.

**Renames write every field that uses a value type,** so a caller not
shown one of them is not granted the preflight, the apply or the
proposal of a value type's migration that renames values, and nothing
is named to it.

#### A migration's refusal names a reading declaration only to an author who is shown it

The preflight refuses a migration that drops or renames a field read by
a declaration in force, as invalid, naming the declaration and the
field (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
The refusal names the reading declaration only to an author who is
shown it, by the rule `describe` uses for that kind of declaration in
force: a proposer without the `schema` grant is not shown a trigger, a
set or a verb whose expression reads a field it may not know of. Such an
author is told that a declaration reads the field, without its name
("after the migration of `invoice`, a declaration reads `note` of
`invoice`, which the migration drops"). The field it names is one the
author knows of: a field hidden from it was not its to leave out, and its
migration does not take it away (above). The refusal stays `invalid`
and can be acted on: the author keeps the field, or asks someone who is
shown the reader. What the refusal names at an acceptance is below.

A view is such a reader too, and it is shown only when its definition
is (above). An author who is not shown the view, by its type or by its
definition, is told that a declaration reads the field it took away, as
above. Where the view could no longer be declared over the migrated
shape for another reason (a field it groups by converted to a kind no
key holds, or to another kind than the merged source it lines up with),
no field is gone, and such an author is told that a declaration over
the type could no longer be declared, without the view and without the
reason. An author who is shown the view keeps the sentences that name
it.

### A report is answered as its reader may know it

Five places answer a migration's report: a proposal's listing, the
preview, and the refusal of a migration that is not lossless, to whoever
proposes, applies or accepts it. One function trims a report, and all
five places call it. Given whole, a report showed a field hidden from an
accepter with its kinds, its counts and its sample keys; the key of a
restricted record to a reader without the grant; a type the reader may
not see among the re-keyed types, the rewritten references and the
colliding keys. The feed and an export are the log as the host reads it,
and carry the report whole.

The rule is the listing's (below). Beyond it:

- **A removed relation** is listed under `dropped` by its own name, to
  one who sees the type and is shown the relation (above).
- **A renamed reference field.** For the migrated type, a reference
  field the same migration renames is listed in `references` under its
  new name (027), and the entry is answered to a reader who may know of
  the field in the shape the migration leaves, as a conversion is.
- **`restated`**: an entry is answered as a conversion is: the type's
  own to one who may see the type, a field's to one who may know of the
  field and, where it was renamed, of the name it had. A member is shown
  as `describe` shows the field: one beyond its shape only to one who
  reads the field in `to` and as the type stands (above). An author kept
  from a field restates nothing of it, since the field is put back as
  declared (above).

Two members only a report that is not lossless holds, and such a report
is never logged:

- **`sample_keys`**: the keys of records the reader is answered. A key
  is a name of a record, and a reader without the `restricted` grant is
  answered a restricted record by no read. So a restricted record's key
  leaves them for such a reader, in a preview and in a refusal alike;
  the counts stay whole, and the report says `more`. They are the first
  five among the records it is answered: the preflight asks before it
  takes a key, so a key held from the reader does not leave four where
  five that it is answered do not convert.
- **`sample_collisions`**: a colliding key is a value of every record
  that would take it, made of the fields of the new key. It is named
  to a reader who is answered two or more of those records, a link
  with its ends: a collision it can see for itself. It also reads every
  field of the new key, in the shape the migration leaves and as the
  type stands; one who reads such a field masked does not read it. A
  record held from the reader that would take a key it is named moves
  the count alone, so the keys named do not tell which one a held
  record takes; a key that one record it is answered and held ones
  would take is counted and not named. The keys named stand in the
  order of the new key, and the sample is the first ten of those that
  may be named: which key a record held from the reader would take,
  and where that record sorts, moves no key in the list. A key shown
  to whoever may see its type told a reader the value of a record it
  is answered by no read, or of a field it reads masked.

A report's id is made over what its reader is shown (above), so a
preview read trimmed is applied by the id it carried, and whether the
migration is lossless is decided over the whole report: the trim changes
what is read, not what is decided.

**`more`.** A report says `more` where an entry was left out of it, or
where a count is larger than the keys named beside it: a field's
`not_convertible` against its `sample_keys`, the `collisions` of the
re-keyed types against `sample_collisions`. Nothing else sets it:
`more` says the counts name more than the samples do; it tells nothing
they do not. Where a key held from the reader sorts changes no answer,
and one who may see everything reads `more` beside seven records that
do not convert and five keys. `collisions` counts records, two or more
a key, so every report that counts a collision says `more`, the owner's
too; how many keys collide is not in the report, and `more` does not
say it. A reader told that a migration is not lossless, and shown no
misfit, reads `more`: the reason lies where it may not look, and one
who may see it reads why. The refusal keeps its kind. A logged and a
stored report never say it: they are lossless and count nothing that is
not named, so a `write_proposed` event and a projection row stay what
they are.

**An author is a reader like any other.** Its migration is completed
with the fields hidden from it, and the report is made over the whole;
what it may not see, it is not answered, in its own preview too.

**One who may not see the migrated type is answered no report.** No
answer that reports on a type's migration is given to one not shown the
type, and its report needs no form with the type taken out: `propose`,
the preview `migrate` and `migrate_apply` answer that the type is not
declared (above), and the accept and the listing answer as for a
proposal that is not there (below).

An accept refused because the migration is no longer lossless carries
the fresh report as the accepter may know it, by the rule of the
listing, and says `more` where that is less than the whole.

#### A value type's report is shown as its listing is

A use says something of a field's values, so the preview of a tightened
value type, the refused apply and a proposal's listing are held the same
way: a use and a misfit are shown where their caller is shown the type
and reads the field, and a misfit names a record: the first misfits
named are of records their reader is answered, as a migration's sample
keys are, so one of a record held from it is counted and not named. The
first ten are taken among those shown: a misfit in a field its reader
does not read takes no place, so the length of the list tells nothing
of that field. The totals stay whole. What the report's id names
follows from who may be shown it: a report its reader may be shown in
part, a tightening's with its uses and misfits, is named by what that
reader is shown, so what it is not shown moves no id; one shown whole
or not at all, a rename's, is named by the whole report, but for its
`conditions`, which stay the reader's (below). Two principals shown
different parts of one tightening therefore hold different ids for it,
and an id handed from one to the other is refused as stale.

- **A proposed tightening whose report does not fit** is refused (013).
  The refusal says how many stored values would not fit and names none:
  a misfit names a record and a field, and the proposer may know of
  neither. A reviewer runs the preflight to see them.
- **A listing shows the report's uses only in fields its reader may
  read,** of types it may see, as a proposed shape is shown. The totals
  stay whole: a count names nothing. A logged report fits, so it names
  no misfit to anyone.

A value type's migration that renames values lists in its report what
may name an old value (027): a set's `where`, a trigger's `when`, a
verb, a view, an expression of a function that holds an old value as a
literal, and an aging rule on a type that uses the value type that moves
from or to one, are listed by their kind and name, to a reader shown
them; another is told that there may be one. A declaration is shown as
`describe` shows it, and an aging rule as a read of its record answers
it, with its `on`, `from` and `to`. The conditions name a function only
to a reviewer shown it (above); to any other it is told only as one
there may be (`conditions_unshown`), as a set or a trigger it is not
shown is.

A value type's report names a declaration by what its reader is
described of it. The report reads the description `describe` gives the
reader and looks for the old value there, so it cannot name what the
description withholds: a type whose old value stands only in a relation
the reader is not shown, in a relation's field or a derived field it may
not read, or in a view's definition held from it, is not named, and
`conditions_unshown` says that a declaration it is not shown may name
one. The rest of the report is what the reader was given before that
member was declared (above).

A proposal's report is listed to each reader as that reader is shown the
lake as it stood before the proposal, its grants followed there through
the renames since, not as its proposer was. The list is the reader's, so
the report's id is too: two readers listed otherwise are given two ids
for one lake.

### A proposal is listed as its reader's view, and one that names what its reader is not shown is not there

A principal with the review grant lists the proposals that are there to
it. Any other principal lists only the proposals it made itself, so a
proposer can follow what became of its write.

#### A proposed write is shown to its reader as a read of the record would show it

`proposals` shows a proposed write to its reader as that reader may read
the record the write is for. A proposal holds values before they are
stored: an upsert's patch, and a call's payload, which is what the
verb's fields become. Shown whole to every reviewer and to the proposer,
they gave a principal that holds `review` the proposed value of a field
every other door hides from it. So the listing shows them as a read of
the record would:

- **A patch** keeps the fields its reader may read. A field the reader
  may not know of leaves it, and so does a masked one: a patch holds
  values and has no place for a name without one.
- **A payload** is shown whole to a reader who may read every field the
  verb sets, and empty to any other. Showing it member by member would
  need the payload paths each assignment's value reads, which the
  declaring rule does not hand out (lakelands#470). For a verb that
  reaches outside the lake, the payload goes to the executor and is
  shown to a reader who may see the verb's type.
- **A write to a type the reader may not see** is not there to that
  reader (below): the type's name and the key are names.

The proposer reads its own proposal by the same rule, whole for as long
as it may read what it wrote.

The names follow the same reader. A migration's report, a rename and a
proposed declaration are shown by what the reader may know of (above).

**Told that there is more.** A listed proposal says `more` when it holds
more than its reader is shown, and says nothing else of it: no name and
no count, since a count would say how many names are hidden. Where the
reader may see nothing of the write, the entry has no write at all: a
write to a record it is not answered (one restricted, without the
grant), a call of a verb on such a record, a bundle none of whose
declarations it would be shown. The entry keeps its id, its proposer,
its times, its state and the proposer's `why`. A proposal that names a
type its reader is not shown is not listed at all (below).

The absent write is a form of the answer and of nothing else. A stored
proposal and a logged `write_proposed` hold the write whole, and
`propose` takes a write, never a proposal; so `Proposal.write` is
optional and `ProposedWrite` has no variant that could be proposed or
logged.

**The filters follow the entry.** `type` and `kind` are answered on the
entry as its reader is shown it. A type the reader may not see answers
what a type that is not declared answers, an empty listing; an entry
without a write is of no kind. The unfiltered listing and `state` hold
every entry the reader is listed. Ordering and paging are over the
entry as shown.

**A proposed bundle** shows each declaration `describe` would show its
reader once the bundle is in force: a type it could see, with the fields
it may know of; a view with its definition or not at all (above); a
trigger and an object set whose condition it may read; a function by
`describe`'s rule; a value type always. A verb is judged by the fields
it names, the ones it sets known and the ones it reads readable, not by
who may call it: a reviewer need not be one who may. One who holds
`schema`, as an accepter of a bundle must, is shown every definition, so
for it `more` can only mean a field or a type hidden from it.

A proposed shape, a migration's or a redeclared type's, is logged as
the store completed it from its proposer's view (above). `proposals`
shows it to each reader without the fields that reader may not know of,
a relation's link fields too: the proposer reads its own view again,
and a reviewer who may see a carried field reads it in the shape and
not under `dropped`.

#### A proposed migration is listed as its reader may know it

A proposed migration holds the type's whole shape and the preflight's
report. `proposals` shows `to` without the fields its reader may not
know of, and each other part where its reader may see the type and may
know of the field:

- **`renames`**: a pair whose old name the reader may know of in the
  type as it stands, and whose new name is in `to` as shown.
- **`dropped` and `fields`**: an entry for a field, or a link field
  named `relation.field`, the reader may know of in the type as it
  stands.
- **`rekeyed`**: the types the reader may see.
- **`references`**: the fields the reader may know of, of types it may
  see.

The totals and `lossless` stay whole, and the entry says `more` when
anything was left out. The two schema hashes are those of the shapes
its reader is shown, and the report's id is made over what it is shown
(above). A logged report is lossless, so it names no record that would
not convert or would collide. A migration of a type its reader may not
see is not there (below).

#### A proposal that names a type its reader is not shown is not there

A proposal that names a type its reader is not shown is not there to
that reader. `proposals` does not list it, and `accept` and `reject`
answer as for a proposal id that does not exist, word for word. The
types a proposal names are:

- the type an upsert or a retirement writes, and the type of a called
  verb;
- of a migration's `to` and of each type a bundle declares: the type
  itself and, of a view, its sources; and the target of each relation
  and reference field it adds, changes or drops against the type in
  force, both compared completed, as the lake keeps a type (each
  computed field's `depends_on` filled, a link field's among them);
- the type each verb, trigger and object set of a bundle is on.

A call of a verb that reads a field its reader may not read is not there
either. Such a verb is not described to that reader, and the proposal
would tell that its `requires` held (above).

A link type is shown only with both its ends (above), so a write to a
link with a hidden end is among them, and so is a bundle that adds a
relation or a reference to a hidden type. A relation or a reference
restated as it stands names nothing the type in force does not. "As it
stands" is equal as declared, in order (link fields and `depends_on`
included), as the lake judges a declaration in force: a restatement that
reorders them counts as a change, and errs toward hiding. An author's
declaration carries back the relations it is not shown, so a
reader not shown a relation's target still proposes, lists and decides
its own redeclaration and migration of the type, which it may author
directly. A type a bundle brings counts as one the lake declares: a
bundle that declares a type its reader would not be shown once in force
is not there to that reader (below).

A migration is judged by its reader under three sights (below): as the
lake stands, as the migration would leave its type, and, where the type
was declared again since, as it stood where the migration was proposed;
its report is trimmed, as above, by the reader's grants there, against
the type as it stood. An accept or a reject judges this as soon as the
proposal is found, before anything else of it is told: whether it is
decided, whether a pinned record moved since it was proposed, whether
its write or its bundle would apply. Nothing is appended. A proposal
hidden only by a record, one restricted or outside a holder's `only`, is
listed without its write (above). A reviewer shown every type it names
decides it, what its grants hide of a field included, where the entry is
its to decide (below).

**Why.** An accept is a write to what the proposal names, and its answer
tells what it judged. If it applied, it wrote to a type its reviewer may
not read, or reshaped one. If it was refused, it named the type, a key
of it, a field and its kind, or that a hidden record moved. A listed
entry whose id the accept then calls absent would tell a proposal hidden
by its type from one hidden by a record. To a principal not shown a
type, the type is one that is not declared (above), and a proposal on a
type that is not there is not there either: not in the listing, and not
at the accept.

#### One function answers what of an entry its reader is listed, under up to three sights

The listing, the accept and the reject each asked whether an entry is
there for its reader, and each by its own lines: the listing trimmed and
left out, the accept asked a second list of the types an entry names,
and a holder of `only` was judged by a third. They agreed where a row
held them together, and nowhere else by construction. So one function
answers. For a reader and an entry it says that the entry is not there,
or what of its write the reader is shown and whether it holds more. The
listing shows that answer, and the accept and the reject ask it
(below).

**Three sights.** The reader is the principal its call named, judged
under up to three sights, its grants followed to each: the lake in force
where the entry is read; the lake as the entry would leave it, a
bundle's whole and of a migration its `to`; and, where the type a
migration reshapes or the verb a call calls was declared again since the
proposal, the lake as it was proposed against. An entry is not there
when any of them hides a type it names, or when its verb, as the lake
declares it or as it was declared where the call was proposed, reads a
field the reader may not read under that sight (above). A call's payload
is shown only to a reader who reads each field the verb sets, under
both. A trigger, a verb and an object set of a bundle name the type they
are on and the type the one in force under their name stands on:
accepted, the bundle would take that one's place, or be refused in words
that name the type, so it is not there to a reader kept from the type it
stands on.

The lake as it was proposed against is read only for an entry whose
subject moved, and only for a reader whose grants keep it from a field.
One who reads every field of every type is shown every type there as in
force, and a migration's names are judged for it on the type in force;
so an owner's listing reads no past for a call or a type's migration,
however many decided migrations the lake holds. A value type's
migration that renames values is listed to every reader as the lake
stood before it (above).

**What follows.**

- A bundle that brings a type its reader would not be shown once in
  force is not there, and so where a declaration of the bundle stands on
  the link type a relation of it calls for: what the bundle declares on
  it is exactly what that reader is kept from. A bundle that declares
  the owner and nothing on its link type is listed to that reader as
  `describe` would show it the owner once in force: the relation without
  the link fields that reader may not know of, with `more` where that
  leaves one out. A principal that proposes a type it will not be shown
  is not listed its own bundle: it is told so on purpose, since the
  entry would otherwise be the one place that names the type to it, and
  the grant is the host's to mend.
- A call whose verb read a field its reader may not read where the call
  was proposed, declared again since without it, is not there
  (above).
- A migration that keys its type by a field its reader may not know of
  is not there: once in force the type would not be shown to that
  reader.
- A migration of a type declared again since, which its reader was not
  shown where it was proposed and is shown now, is not there: the entry
  would say that the type was there and that someone proposed to
  reshape it, which a read as of that position does not answer that
  reader.
- A migration of a type declared again since is judged, for a reader
  masked on a field and kept from none, on the type as it was proposed
  against, as for a reader kept from a field.

**Why.** Two answers to one question differ sooner or later, and here
each difference told something: an accept that applies what the listing
left out writes a type its reviewer may not see, and one that refuses
what the listing never showed says that it is there. The past is asked
because a declaration made again can take away the very thing that hid
an entry, and the entry is of the lake it was proposed against.

**The rows.** A grid of sixteen entries, an id that was never proposed
among them, by six reviewers pins, for each pair, how the entry is
listed and what its accept answers, and that the reject of one not
listed or not granted answers as its accept, the head unmoved.

### An accept decides only what its reviewer is shown, and its refusal names only what the listing shows

A principal with the review grant may accept or reject a proposal that
is there to it, as the rules here say; any other principal decides
none. The accept and the reject ask the listing's answer (above) at the
head as soon as the proposal is found: an entry that is not there is an id
that does not exist, word for word, to both; one listed without its
write for a record its reviewer is not answered, or to a holder of
`only` not whole (below), is not granted to decide; and an accept's
refusal is told by the same answer (below). A bundle that a reviewer
without `schema` who holds no `only` is listed without its write, none
of its declarations shown, is that reviewer's to reject; its accept
takes `schema`.

Accepting a proposal applies it in the proposer's name and not under its
grant, which is not in the log (below): the write was checked whole
under the proposer's grants when it was proposed, and the accept plans
it again as the proposer. A reviewer need not know all of what it
accepts: the accept asks for `review`, and for `schema` where the
ontology changes, and applies the write as it was logged and checked
under the proposer's grants, the part its reviewer was not shown too.
Trimming what is applied to what the reviewer read would make the
reviewer's grants decide what the proposer wrote, and a write checked
whole would be applied in part. A reviewer shown every type an entry
names applies the write whole, the part its listing trims included,
where the entry is its to decide (above).

Accepting a change of the ontology needs `schema` as well as `review`.
For a record write, the proposer's own grants were the check when it was
proposed, and its reviewer decides whether it happens, where the entry
is its to decide: a holder of `only` is judged by its own pairs too, and
a reviewer without `restricted` neither accepts nor rejects a write that
reaches a held record (below). A change of the ontology is one the
proposer usually could not make, so the lake's grant for it decides:
otherwise any proposer and any reviewer together could change the
ontology though neither holds that grant.

An accepter is answered as its listing shows it: a proposal on a type
it is not shown is to its accept an id that does not exist, and so is a
proposed trigger, verb or set under the name of one that stands on such
a type: its accept would take the place of the trigger or the verb, and
be refused for the set in words that name the type (above). A proposal
made while a name was free and accepted, after that name was declared on
a type its proposer is not shown, by a reviewer shown both is that
reviewer's to decide: it is applied, as its proposer's. And an accept
passes the line of the tools' names (above) with its accepter as the one
asked: a proposal, of a set or of a migration, whose tool's name was
taken since by what its accepter is not shown is not granted to it,
naming nothing, where the lake's own check would name it; an accepter
shown both is refused, naming both. Its reject passes no such line, as
it stands: the accepter is listed the proposal, with what it is not
shown left out, and rejects what it is not granted to accept.

#### An accept's refusal names what its reviewer's listing shows

An accept applies the proposer's write, planned again as the proposer,
so a refusal of that plan is the proposer's: it may name a field, a
type, a key or a value the reviewer is not shown. What an accept
answers a reviewer is judged by the reviewer's view, as its listing is.
A refusal of the write it applies (an upsert's plan, a verb's run) is
told whole where the reviewer is shown each thing it names:

- each type, one the reviewer is shown;
- each field, one it reads of the type written; a verb named counts the
  fields its `requires` reads;
- each verb, the one called;
- what a field's declaration says (the values it allows, its value
  type), only beside a field named;
- each key, the one written or, as each value, one the listing shows
  under a field named, or in a call's payload, which is listed only
  whole.

A call of a verb whose `requires` or set values read a field the
reviewer is kept from does not come to this. It is not there to that
reviewer (above).

A name of another kind, a relation's, a trigger's or a function's, is
not judged. Any refusal is told whole to a reviewer that reads
everything. The proposer is judged as any reviewer, by its grants at the
accept: it named those things when it proposed, but a grant narrowed
since keeps them from it now, and its own write would tell it no more
today. Otherwise the accept answers as a proposal stale since it was
proposed: the write changed since it was proposed, naming only the type
and the key written, which the listing shows. A conflict names versions
only, and is told whole. A retirement's refusal is told as its
reviewer's listing shows the write, as an upsert's and a call's are;
what a write sets off is judged by declarations the lake already took,
so that is not judged. A refusal is judged by what its
case's members are, as typed (`Named` in the contract). A link's
cardinality is judged only where a link is made, by `link` and by
ingest, so no accept judges one; what a proposal does to a link is
`013 § A proposal makes or retires no link, and the accept of a call on a link judges the link and both ends again`.
A reject answers nothing of the write.

**Why.** The listing shows a reviewer the proposal trimmed to what it
may know, and says `more` where that is less than the whole. A refusal
that names a hidden field, its target type and the key it held would
tell the reviewer what the listing withheld, through the door the
reviewer is meant to use on it. A write that no longer applies needs no
more than that answer to be proposed again, by one who may see what
changed.

**A migration's refusal at the accept** follows the same rule. The
refusal of a migration that takes away a field a declaration reads
(above) goes, at an acceptance, to the one who accepts, who holds the
`schema` grant and is shown every trigger, set and view whose type it is
shown; a verb is shown, there as anywhere, to one who may call it. An
accept refused because a declaration made since reads a field the
migration takes away names the declaration and the field to an accepter
who may know of the field, and to another says only that a declaration
reads a field the migration takes away: the proposer took the field away
knowingly, the accepter may not know that it exists.

### A grant may admit only the records that hold its pairs

A principal's grant on a type may hold `only`: pairs of a field and a
value. Such a principal, a holder, is admitted only the records whose
fields hold every pair, and every other record of the type is, to it, a
record that does not exist.

- **Identity, text, closed.** A pair holds where the stored value is the
  same JSON value, as `Scan.equal` judges it. A pair must name a field
  the type declares, of kind `text` or `reference`, that the holder may
  read, with a text value. A grant that names anything else is not
  applied and not guessed at: the doors that name the type refuse it as
  invalid, naming the type and never the field, until the host fixes the
  grant; a walk or a view that reaches the type admits none of its
  records. Nor does any other path, whatever a stored value holds: the
  proposal listing answers a proposal on such a record without its
  write, a reference into the type, a link's end too, names no live
  record, as an absent key does, and a write on a record of the type is
  one on another's. A value equal to the pair would otherwise tell which
  records hold a field the holder may not read. Two names of one field
  that a rename joins with differing values hold null, which no pair may
  hold.
- **Every read.** `get` of another's record is not found, as a missing
  one is. `find` with a filter, an order, `total`, a count, a sum and a
  group read the engine under the pairs, so they count and fold the
  holder's records alone. A search ranks the type and keeps the admitted
  candidates, pulling deeper until its page is full; the fused search's
  path that reads no record is not taken. A walk reaches only admitted
  records. A proposal on a record the holder is not admitted, before the
  write or after it as proposed, is listed without its write (above); it
  still shows why, the proposer and a type-level verb's payload, as one
  on a restricted record does. A read as of a position judges the
  records as they stood then, under the grant followed to that position.
- **A search as of a position** keeps a candidate only where it is
  admitted then and at the head too, under the grant followed to the
  head and by the name the type bears there: the ranking reads the text
  and vectors at the head, and another's current text must find nothing.
  A grant that cannot be applied to the head's declaration admits no
  candidate there, though the stored values stay. A cursor page and a
  query started by a search keep working.
- **Views.** A view's effective `only` is its own pairs, plus, where it
  has one stored source, no merge, and groups by every field the
  source's pairs name, the source's pairs on its group fields: each
  group's records then share those values, and the admitted records are
  the groups that hold them, so a kept view stays kept and is read under
  the pairs. Otherwise a view that reads a type held to some records, or
  a link type whose end type is, is computed from the admitted records
  alone. `describe_type` says `only some` of such a view to its holder,
  never whether every owner's groups are exact.
- **Links.** A link record is withheld from a holder of `only` on an end
  type unless that end is admitted; its ends are read once per page of
  links. `only` on a link type admits its links by their fields, in a
  walk too. An end that is itself a link is judged as that link is.
  Readers holding no `only` on any end, however deep, are not judged by
  this rule.
- **A holder writes its own records alone.** A write is judged against
  the record the holder owns at the key: the record there, retired or
  not, where its grant admits it, and otherwise none. Another's record
  is, to the holder's write, a key that is free, so every check (the
  expected version, the key fields, a reference, a required field)
  answers as it would on a free key. A write that passes them all is
  refused as not granted, naming no field, where another's live record
  stands at the key or the record the write leaves is not admitted: a
  free key with another's owner, another's key and a handover answer
  alike. `retire`, `unlink` and a verb read the record first: another's
  is not found, as a missing one is. A link is the holder's where each
  end it holds `only` on is admitted. `ingest` compares a re-delivery
  only against the holder's own record, so a re-delivery onto another's
  key is refused as on a free key, not counted a duplicate.
- **A holder proposes writes to its own records, pinned to them.** Its
  `propose` of an upsert, a retirement or a call, and a call of a verb
  above low risk, which is a proposal, are judged as the direct write
  is, against the record it owns at the key. The proposal is logged
  `pinned` where the holder holds `only` on the type it writes, a link
  type with a held end included (`docs/log-format.md` §2.4). The accept
  of a pinned proposal is refused as invalid, the proposal left pending,
  once the record at its key has an event after the proposal's position,
  live or retired, or, for a link, either end record has one (a link is
  its holder's by its ends, and a change to an end moves no link), or,
  for a call, a record it reads across a reference has one (055), or
  the type was declared or migrated since, a re-key included: a rederive
  and a backed output move the record without a write, and a record made
  again at the key starts its versions anew, so the position, not the
  version, is what the accept compares. So the accept applies the write
  to the record and type its proposer was judged on. The proposer's
  grant is not in the log, so a holder may propose only where nothing
  but the write can move its pairs: a proposal is refused where a field
  its `only` names is derived or backed, or where the verb assigns one.
  A reference a pinned proposal writes is judged at the accept as a live
  record, as for any proposal: it names a key the proposer knew when it
  proposed. The accept writes in the proposer's name, not under its
  grant, and the pin compares records and types, not grants: a grant
  narrowed or revoked between the proposal and its accept does not stop
  a pinned proposal. The reviewer decides it, on what the listing shows.
- **A reviewer holding `only` decides what it is shown whole.** It
  accepts or rejects a proposal on a type it holds `only` on only where
  the proposal listing shows it the proposal whole: the record at the
  key, before the write and after it as proposed, admitted, and every
  value the write carries one it may read. Otherwise it is refused as
  not granted before it is told whether the proposal is decided, so a
  proposal id, which is positional and so can be guessed, tells it
  nothing the listing does not. Its accept is judged by its own pairs
  too: a write that would leave the record outside them is refused as
  not granted, as its own write would be.
- **What is refused**, as not granted, naming no field: an aging rule on
  the type, which moves records at the host's run. A holder of `only` on
  any type changes no shape: every schema door and the proposal of a
  declaration or a migration are refused to it, and no migration's
  proposal, of a type or a value type, is listed to it, since a report
  counts every owner's records; its accept and its reject answer as an
  id that does not exist, as for any entry that is not listed (above): a
  refusal as not granted would tell that the id is a proposal's. A
  proposed declaration is listed and not granted to decide. A reference
  written into the type from another type must name an admitted record.
- **`describe`** tells the holder its pairs in `may.only`, where the
  grant can be applied, and offers no write, link or verb that would be
  refused: an upsert only where it writes every field its `only` names
  at the authored door, so that a create can succeed, and no verb above
  low risk that it may not propose. A field's `initial` value is not
  counted there: where only an `initial` value would make a new record
  the holder's, the write is admitted (the record it leaves is judged,
  `initial` values included) but no upsert is offered. The change feed
  is the log unmasked, so a holder does not read everything and is
  refused it at a door.

`only` is a function of `Principal.id` within a lake: a host gives each
owner its own id and never gives one id `only` on some calls and not on
others. A retry is answered its first outcome before the grants are
asked, as every retry is; under this contract a holder's earlier call
was refused, and its retry answers that refusal.

**Why.** A host often needs "each sees only their own": an employee
their claims, a customer their invoices. Filtering above the lake works
record by record but not for a sum, a group, a search, a walk or a view,
so such doors had to be refused instead of answered. The store reads
every record through two places, the engine's scan and the check of a
record read by key, and both take the pairs: the engine selects only
admitted records, so every answer built from a scan sees only them, and
no door filters after the fact.

The value is in the grant, not an attribute name: the host's resolver
already builds the grants per call and writes the owner's value in, so
the store never decides which attribute of a principal counts. Text
identity keeps the rule one comparison every engine already makes, with
no decimal or float spelling to agree on, and lets the engines narrow by
an index.

A holder's write is judged against its own record, never another's,
so that no answer it gets is made from a record it may not read; the
refusal of a write onto another's key comes last, where a free key
would have been written, and that one difference is the existence a
create cannot help showing. A proposal waits for a later accept that
re-judges the write under no grant of the proposer's: the pin makes the
accept apply the write to the record and the type its proposer was
judged on, or refuse it. The pin is one bit of the proposer's grant in
a log that is not masked: whoever reads the log learns that the
proposer held `only` on the type, never on what. A reviewer is a reader
of what it decides, so it decides only what it may read, and what it
accepts it is answered for as for its own write.

#### `only` keeps the reference it names

A reference to a type its principal is not shown is a field it may not
know of (above). A field the effective `only` of a type names is
kept as granted: its value is the pair the host wrote into the grant, so
it tells the holder nothing, and `only` stays one that can be applied.
The effective `only` counts, a view's included: a view grouped by its
source's `only` field keeps that field, and so its key, and does not
vanish (above). The type a grant names, and the value it holds a field
to, are the host's own choice: naming a type in a grant shows its name
to no one.

#### One answer says what a grant admits, and one whether a record is answered

What `only` admits of a type's records, and whether a record is
answered: each question has one answer, in the sight (above).

What a grant admits is every record, the records that hold its pairs, a
view's records computed from the admitted ones and held to its pairs, or
nothing. A grant that cannot be applied admits nothing, and no reading
of the answer makes every record of an empty list or of a missing one: a
door reaches the pairs through the answer alone. A record is answered
where it is not held from the principal (below) and its grant admits it,
and a link with its ends: each end whose type the principal holds `only`
on is admitted, retired or not, and no end is held from it. A read, a
write of the record, a reference to it, a relation's cardinality, an
aging rule in a rename's report, the listing of a proposed write, with
the accept and the reject that go by it, and a migration's report ask
these.

Three doors judge a link as a read of it does. A reference to a link
whose end the writer's `only` does not admit is refused as one to a link
no record holds is. A proposed call on such a link is listed without its
write to a reviewer who is not answered the link, and is no more that
reviewer's to decide than any proposal it is not listed whole; its
accept is refused for a verb that writes the link and for one that
reaches outside the lake. The hold has the same side: a proposed call on
a link with a held end is listed without its write to a reviewer without
`restricted`. A migration's report names, among its sample keys, neither
a link with an end held from its reader nor the links of a link type its
reader is not shown, and its counts stay whole (above).

A relation may point at a link type, and a link of it has an end that is
a link. An end that is a link is there where that link is, by its own
ends, as deep as they go: a link's key is made of its ends' keys, so the
descent ends with the records. Whether a link type is judged by its ends
at all is asked the same way through its declarations, each link type
once, since a migration can point two link types at each other. A write
that names such a link follows it as far: a second link where a relation
allows one names the first only where its writer is answered it with its
ends; the records a proposed write reaches, by which its reviewer's
decision and its proposer's `restricted` grant are judged, are a link's
ends and theirs; and a pinned proposal on such a link is accepted no
more after a change to any of them.

**Why.** A judgement stated twice is two rules once one of them
changes, and the one left behind hands out what the other keeps: each
of the three doors was a copy with a part missing. An answer that fails
closed cannot be skipped: a site that asks nothing has no pairs to
filter by, and so no record to hand out.

### A record held at the head is not there to one without the grant, at any position and at every write door

A retention hold withholds a record without erasing it. The host places
one through the store facade's `restrict`, with its reason, and lifts it
through `unrestrict`; no caller's door writes either
(`docs/log-format.md` §2.3). To a principal without the `restricted`
grant a record under a hold, a restricted record, is not there: `get`
does not find it, as a missing record is not found, and `find`, a
`total`, a count, a view, a walk, a graph and a search leave it out. To
a principal with the grant it is answered with its fields unchanged, its
history included, and lifting the hold brings it back for every reader,
at every position.

What the other records say of a held record follows from that. An
engine's full-text and vector candidates include restricted records, and
the store applies the principal's grants to the hits
(`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`),
so one without the grant is never answered it as a hit, and it moves no
other hit's rank or score. A restricted link is walked only by a
principal holding the grant
(`docs/decisions/035-a-link-is-a-record.md`). In a view the fold keeps,
a restricted record is a member for a reader with the grant and for no
other (`docs/decisions/049-a-view-the-fold-keeps.md`). The change feed
is the log unmasked, and a principal without `restricted` is refused it
at a network door before the stream starts
(`docs/decisions/037-the-feed-over-a-network-door.md`).

The suite holds the rule in `a_restricted_record_is_withheld_but_kept`
(`get`, without the grant and with it, and the hold lifted),
`a_count_answers_as_many_records_as_a_full_find_walk` (a count),
`a_retired_or_restricted_record_is_never_a_hit`,
`a_record_the_read_may_not_see_moves_no_hybrid_score` and
`an_axis_counts_only_what_its_reader_is_shown` (a search), and
`a_restricted_link_is_walked_only_with_the_restricted_grant`,
`degree_counts_each_records_links_to_ends_it_may_see_highest_first` and
`a_graph_shows_a_reader_no_more_than_its_rows` (a walk, a degree and a
graph). Two hold it by comparison, while a record is held: in
`a_page_answers_its_total_and_an_offset_jumps_to_the_walks_slice` a
`total` equals what its reader's own walk finds, and in
`a_kept_view_of_each_shape_answers_every_read_as_its_twin_computes` a
kept view answers a reader as its computed twin does. That a holder of
the grant reads a held record's history is asserted in
`a_record_held_across_a_rekey_is_not_there_as_of_before`: its `total` as
of before the re-key is two above the reader's, the two held records.
The parts below are held in
`a_record_held_now_is_not_there_at_any_position`,
`a_record_held_across_a_rekey_is_not_there_as_of_before`,
`a_restricted_record_is_not_there_to_a_writer_without_the_grant` and
`a_link_with_a_held_end_is_not_there_and_an_accept_is_judged_by_what_it_reaches`.

#### The hold that stands at the head governs every position

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
is never released by not being found. Every door then judges the hold
as it does at the head. A page the engine fills with records held now
is read on until it is full, a total is counted in pages, and a kept
view, folded as it stood then, is computed instead, and `describe` says
it is not kept here. A read at the head, or by a principal with the
grant, reads through no such view.

**Why.** A hold withholds a record from those without the grant. A read
as of the moment before the hold would hand it back whole, so a hold
that cut only the head would withhold nothing. A record released from a
hold is one its readers may see, and the history before its release is
part of it.

#### A restricted record is not there to a write

A writer that may not read a restricted record must not change it, point
at it, or learn its version. So the record is, to such a writer, not
there at any write door, as another's record is to a holder of `only`. A
reference to it and either end of a `link` are refused `not_live`, as
for a key no record holds; `retire`, a verb called on it, and `unlink`
of a link with a restricted end are not found; an upsert onto its key is
refused as not granted, naming no field, so the key is told as a create
tells it and the held record is neither written nor its version shown; a
`link` onto a link whose own record is held and an ingest onto a live
held key are refused the same way. A retired record that is held counts
as another's too, so no upsert, ingest or proposal writes into it, and
an ingest judges a link's ends as `retire` does. A proposal of an
upsert, a retirement or a call is judged as its write. Its accept judges
the write as the records stand then. The proposer's grant is not logged;
a proposal that reached a record restricted when it was proposed could
only be a holder's, and is accepted as one; any other is judged without
the grant, so a record restricted since the proposal is not there to its
accept.

A change of shape names no record: a migration, a value type's
migration and a declaration convert, re-key and rename every stored
record of the type by the fold, a held one with the rest, whoever makes
them (`027 § A migration converts history too`); of a held record their
reports give its count, and its key to no reader without the grant
(`006 § A report is answered as its reader may know it`).

#### A link with a held end is not there, and an accept is judged by what its write reaches

A link with an end its reader is not answered is not answered either, at
`get` and `find` of the link type, at a reference to it and at its
writes: a link from or to a held record is, to a reader without
`restricted`, a link no record holds. A view over a link type is
computed for such a reader, as for a holder of `only`: no kept face
judges a link's ends. A relation's cardinality still holds over every
link, whoever can see it: a refusal for it names the other link's end
only where the writer is answered the link that holds: by its ends,
under `only` and `restricted` alike, and by its `only` on the link type
itself, from either end; so what is kept stays whole before it stays
hidden.
What a proposed write reaches is the record it writes, a link's two end
records, and the records its references name: an upsert's, those its
record computes (032), and a call's as its assignments compute them for
the record at the accept, whatever form they take, with the records its
`requires` and set values read across a reference (055), as when it was
proposed and as at the accept; and the two end records of each link
among them. A reviewer without `restricted` is refused, as not granted,
the accept of a write whose own record or ends are held, and its reject
as its accept: a reject that passed beside a refused accept would tell
its reviewer that the write reaches a held record. A proposal that names
a record its reviewer is not answered beside its own (a reference its
upsert sends or computes, a record its call reads across, and their ends
where one is a link) is not there to that reviewer, held when it was
proposed or since: not listed, its accept and reject answered as an id
no proposal holds, as one whose verb reads a field its reviewer may not
read is. This is asked of a reviewer without `restricted` and of one
holding `only`, by the one function that computes the reach. The
accept judges a proposer as a holder where its write reached one when
proposed, computed wholly as it stood then (the declarations, its
record, the settings in force and the proposal's time): a record held, a
field retyped, a setting changed or a time passed only since is no proof
of the grant. A call above low risk is judged as its caller's write
before it is proposed, as `propose` judges it. The host places and lifts
a hold on a retired record too: a key whose retired record is held is
otherwise closed to every writer without the grant for good.

### Some things are still told, on purpose

What remains says a hidden thing exists, and remains on purpose:

- the version, which moves on every change, a hidden one included
  (above);
- the key, the valid times and a relation's `via`, which are the shape
  and are given of a masked field too. A key field the reader may not
  know of hides the type instead (above), since the key would give its
  value. A valid time naming such a field is not named: `describe`,
  `describe_type`, a listed proposal and a migration's report give the
  type without that bound, and a read at a `valid_at` answers as without
  it (above);
- a valid time sent where one the author may not know of stands, which
  is refused as taken, as a field named like a hidden one is (above). A
  name no field has, or a hidden one, sent there answers as on a type
  with no valid time: only a field the author may name tells it, as only
  a taken name does;
- an inverse sent onto a type where a relation the author is not shown
  bears it, which is refused as taken
  (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
  An author, a proposer without `schema` and a reviewer at an accept
  learn a yes or a no for a pair of inverse and target, and neither the
  relation's name nor its owner;
- a verb on a relation's link type, to a caller who is not shown the
  verb, kept from the link type or not, where a migration it sends or
  accepts removes the relation. The removal is refused
  (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`), in a
  case of its own that names the relation and the link type its name
  gives, and names the verb to an author `describe` shows it to, one who
  may call it. A caller who is not shown the verb, one kept from the
  link type itself among them, is answered the same case without the
  verb: that a declaration is declared on the link type, at the
  preflight, the apply, a proposal and, as a reviewer, the accept. The
  refusal must say why the migration cannot go. The relation is one the
  caller is shown, and the link type's name follows from the relation's.
  Since only a verb refuses a removal, the caller learns that a verb
  stands there; it learns neither the verb's name, nor how many there
  are, nor what one sets or requires;
- `may.retire`, which says every field is writable.

**What stays.** A principal with `schema` is shown every definition, and
a write answers `through`, the position its consequences reach. One who
is shown a condition and sees that a write reached further than itself
can infer that the condition held, where nothing else explains it. The
lake answers no name and no value; it cannot hide that something reacted
from one who must read its own write's consequences. So `schema` is not
handed to a principal that is kept from a field or a type
(`docs/hosting.md`).

**What a holder's write tells.** A create succeeds on a free key and is
refused on another's: a key's existence shows where a write would make
something, and a key that carries the owner (the owner field among the
key fields) keeps holders' keys apart. A create on a key whose record
another retired continues that record's version, and takes its
`restricted` mark; a holder without that grant makes no record on a
held key (above). `ingest` judges a link's cardinality only once the
record its writer leaves is admitted: a link from another's record
answers as one from a free key, and a link a grant's `only` on the link
type admits by its own fields holds the relation's cardinality as any
other.

**What a record a caller is not answered tells.** It is accepted under
`restricted` as under `only`: a cardinality refusal and an accept's
refusal each say that a record the caller is not answered exists.

**A value type's renames count every record.** Their counts stay whole,
as a report's totals do (above): a field's count per pair holds the
hidden and restricted records too, so it tells a caller how many records
it is not answered hold a value.

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

Masking the answer does not protect a value the question already
revealed. A filter `salary > 100000` answers, record by record, whether
the hidden salary is above the line; a sum over a group of one is that
record's salary; an ordering ranks the hidden values. Each of these
turns a masked field into an oracle. Refusing the question is the only
answer that leaks nothing.

## What it is not

- It is not an off switch. There is no mode in which the store stops
  checking grants; a fully trusted principal is one whose grants cover
  everything.
- It is not authentication. Who the caller is gets decided before the
  store is called, by the resolver.
- It is not partial success. A write is never applied with the
  forbidden fields silently dropped.

Of a probe:

- The rule does not refuse reads of records that merely have such a
  field. A record is still returned with the field masked or absent.
- It does not decide what a writer may write. A write is governed by
  the principal's write grants (012); a verb is held to what it reads
  as well, and what a write's refusal may tell is a rule of its own
  (both above).

Of `only`:

- It is not several admitted values for one field, nor a predicate
  other than equality.
- It is not taken from `Principal.id` implicitly.
- It does not change `restricted`: a record must pass both rules.
- A reference value naming another's key is shown as a key; the record
  behind it is not found, as for a restricted record.

## How it came to be

- The first version: a call carries one principal with grants per
  field, and the store enforces them on every read and write. In 013: a
  principal with `review` lists every proposal, any other its own. In
  024: a read that filters on, orders by or aggregates over a field
  needs the right to read it.
- 024 tightened `describe` in 0.4.0: a masked field by its shape alone,
  and no definition that names a field its reader may not read
  (lakelands#206).
- 013 added: a declaration or a migration can be proposed, and its
  accept takes `schema` beside `review` (lakelands#199).
- 024 added in 0.5.0: a search probes what it searches in every mode,
  and a vector what it is embedded from (lakelands#242); a search
  limited to named fields probes those (lakelands#236); a field its
  reader may not read is described without its annotations, and a
  relation's link field by its shape alone, which closed a gap:
  `describe` had answered each relation whole (lakelands#239).
- 013 added: a verb called on its type takes its type's grant
  (lakelands#277).
- 024 added: a read at `valid_at` probes the type's valid-time fields
  (lakelands#372); a write checks grants before it reads the data they
  protect, and the version right after the record is found
  (lakelands#343); a field built from another is read as that field
  (lakelands#387).
- Added: what `none` and `masked` each say, and the answer a probe or a
  write gets that names a hidden field (lakelands#394); `write_only`,
  which ended the one ladder `none < masked < read < write` that access
  was until 0.8.0, and gave a host that matches on the access enum one
  more arm (lakelands#393).
- 027 added: a shape is its author's view, 024 that a hidden field is
  not removed by one who may not know of it, and 013 that a proposed
  shape is listed to each reader without the fields it may not know of
  (lakelands#475); 027 that a migration's refusal names a reading
  declaration, and a view, only to an author who is shown it
  (lakelands#495, lakelands#504).
- 024 added: a view's own probe names nothing (lakelands#505).
- 013, 024 and 027 added: a proposal is listed as its reader's view,
  its patch, its payload, its shape and its report trimmed, and says
  `more`; before, `proposals` showed every reviewer and the proposer
  the write whole (lakelands#499).
- 013 added: a value type's tightening can be proposed, its refusal
  naming no misfit and its listing no use its reader may not read
  (lakelands#476).
- 027, 024 and 013 added: a report is answered as its reader may know
  it, by one function in five places, its sample keys those of records
  its reader is answered, and in 013 an accept refused as no longer
  lossless carries the report as its accepter may know it
  (lakelands#531).
- 024 added: a computed field is read as what it is computed from,
  listed or not (lakelands#540).
- 024 added: a declaration reads only what its author may read, a
  write's answer names a reaction to one who reads its condition, and
  a value type's report is shown as its listing is, where the preview
  of a tightened value type had listed every type and field that uses
  it, with its counts, and the refused apply named the first values
  that would not fit, to a caller that was shown neither the type nor
  the field; 027 that a type its author is not shown is a type that is
  not declared (lakelands#538).
- Added: a grant is followed through a field's renames, where one that
  stayed on the old name had let the field fall to the type's default,
  with 024's sentence on the same field under another name
  (lakelands#527); a type's names are followed too (lakelands#429).
- 024 added: a named set is read by its filter's right (lakelands#552).
- 027 added: a renamed reference field's entry in a report is answered
  as a conversion is (lakelands#524).
- 024 added: a field's declaration is shown by one list, wherever it is
  answered, where the `to` of a listing had handed out a masked field's
  members since before `restated` existed; 027 the rights of a report's
  `restated` (lakelands#554).
- Added: a proposal that waited is listed by the names it was written
  with (lakelands#594).
- 024 added: a setting the store reads is judged by what its writer is
  shown (lakelands#601).
- Added: a link type and a view are shown only with what they are made
  of, where each had been judged by the grants on its own name alone
  (lakelands#595); the first question of every door that writes a
  record, whether its writer is shown the type, in 024 with what was
  measured (lakelands#607); the raise by `sensitive` stopped counting as
  access to a type, which narrowed "raises an inherited `none` to
  `masked`" to one who is shown the type (lakelands#608); `may.find` and
  `may.get` are said of every type `describe` answers, where they had
  been said only of a type its principal reads a field of
  (lakelands#620).
- 051 was written: a grant may admit only the records that hold its
  pairs, and every write that reaches the type was refused
  (lakelands#634).
- 027 added: a read shares the shape of its range, and nothing of a
  reader is in what is shared (lakelands#557).
- 024 added: a schema hash is that of the shape its reader is shown,
  and a report's id is made over what is shown (lakelands#644); an
  expression is read as it is checked (lakelands#652).
- Added: a pattern's probes ask whether a type is shown (lakelands#653,
  lakelands#654); a relation is declared only to one shown both its
  ends, a link type is made of its relation's target too, a search that
  names no fields searches those its principal reads, in 024 the rest
  of a description is computed over the shown shape, and in 027 the
  author's view keeps a relation its author is not shown
  (lakelands#662).
- 051 added: a holder writes its own records alone and proposes writes
  to them, pinned, where every write that reached the type had been
  refused (lakelands#660).
- 027 added: a value type's migration lists what may name an old value
  to a reader shown it, and is not granted to one not shown every field
  it writes (lakelands#553).
- A key field held at `none` hides its type, where it had hidden the
  field and answered its value as the key (lakelands#715).
- 024 added: a relation carried by a field is walked by its reader only,
  a change for a host whose reader walked it before, and 027 that under
  the author's view a shape that sends a relation of a hidden one's name
  declares it twice (lakelands#727); 024 that a valid time over a field
  its reader may not know of is not named, and 027 that what a shape
  names stands as declared only where it stands, where a member that
  named a hidden field had been taken because the field existed in the
  lake, which told the author the name (lakelands#726); a database's
  error quotes no row (lakelands#735, lakelands#739); a hybrid search is
  judged before it embeds (lakelands#741).
- 006, 024 and 051 added: a reference to a type not shown is a field
  not known of, a field `only` names excepted; before, a reference's
  target was kept shown on purpose, so that a field's access would not
  depend on another type; and 027 that a reference field to a type its
  author is not shown is kept as a hidden field is, where an unchanged
  one stood as declared (lakelands#718). In 027: a reference an author
  sends to a type it is not shown is refused as to one that is not
  declared (lakelands#716).
- 027 added: a hidden name is judged at the step an absent one is
  (lakelands#750).
- In 024: a valid time sent where a hidden one stands, refused as
  taken, joined what is told on purpose; in 027: a valid time its
  author sends is judged by its name first, where one equal to the one
  in force stood as declared (lakelands#692).
- 013 added: a proposal that names a type its reader is not shown is
  not there, where 024 and 027 had listed it without its write
  (lakelands#762).
- Added: nothing beside a type names it to one it is kept from, so
  `describe` agrees with `call` and `propose` on a verb (lakelands#770).
- 051 added: an `only` that cannot be applied admits no record on any
  path (lakelands#771); a restricted record is not there to a write
  (lakelands#772).
- 024 added: a verb reads only what its caller may read, and 013 that a
  proposed call of a verb that reads a field its reader may not read is
  not there (lakelands#779). 013 added: an accept's refusal names what
  its reviewer's listing shows (lakelands#780). 051 added: a link with a
  held end is not there, and an accept is judged by what its write
  reaches (lakelands#781).
- Added: a view's key is judged at its sources too (lakelands#782), and
  a view declares no key but its groups (lakelands#788); a pattern's
  path on a type not shown is refused as one no bound name starts
  (lakelands#752).
- Added: a hold hides a record at every position, not only at the head
  (lakelands#790).
- The list of renames became the fold's recorded output `sys:renames`
  (lakelands#823).
- Added: one sight answers what a principal is shown, at every door;
  before, a reviewer who knew of a valid-time field and might not read
  it was listed the entry with no write at a valid time, and whole
  without one. In 024: a view's definition is shown to those its read
  answers, by one rule for `describe` and the read (lakelands#798).
- 024 added: a step with a `type` probes what it keeps, where a step
  whose `type` it does not reach had been refused as not granted when a
  relation of its name was masked, and an inverse refused as taken among
  what is told on purpose (lakelands#829); 024 and 027 what is told of a
  verb on a link type whose relation a migration removes
  (lakelands#843).
- The bundle's listing of a view whose definition its reader may not
  read got its row (lakelands#847).
- 013 added: an entry is decided as it is listed, by one function and
  up to three sights, and at an accept's refusal the proposer is judged
  as any reviewer (lakelands#791). Before, a bundle bringing a type its
  reader would not be shown was listed without it and its accept applied
  the whole; a call whose verb was declared again without what it read
  was listed whole; a migration keying its type by a hidden field was
  listed trimmed; and a holder of `only` was refused a migration's
  accept as not granted (lakelands#798, lakelands#791). In 024: a set
  named in another's filter is judged by the sets its author is shown,
  and in 027 an object set its proposer is not shown is not granted to
  propose (lakelands#877).
- 051 added: one answer of what a grant admits and one of whether a
  record is answered, a link with its ends. Before, what `only` admits
  of a type's records was worked out where it was asked: a read took the
  effective pairs and checked beside them whether the grant can be
  applied, a write asked the type's own pairs, and two sites took the
  bare pairs, where an empty list also meant every record; whether a
  record is answered was stated seven times, each judging the hold, the
  pairs and a link's ends by its own means, and some left a part out.
  Three doors judged a link by its record alone: a reference to a link
  whose end the writer's `only` does not admit was accepted; a proposed
  call on such a link was listed whole to a reviewer who is not answered
  the link, who could reject it though its accept was refused, and its
  accept ran for a verb that reaches outside the lake; a proposed call
  on a link with a held end was listed whole to a reviewer without
  `restricted`, whose accept and reject of it were refused; and a
  migration's report named, among its sample keys, a link with an end
  held from its reader and the links of a link type its reader is not
  shown (lakelands#798, lakelands#871).
- 051 added: an end that is a link is judged as that link is, where it
  was judged by its own record alone: a holder of `only` on a type two
  links down was answered the link onto another's link at every read,
  with `restricted` or without, and so was a reader without `restricted`
  the link onto the link of a held record; and the refusal of a second
  link named the other end of a link that `only` on the link type itself
  kept from its writer, whose `get` of that link was not found
  (lakelands#879).
- 024 added: a walk at a valid time probes every type it can meet
  (lakelands#831), and a view computed at a valid time its sources'
  valid time (lakelands#881).
- 027 and 024 added: a trigger, a verb, a function that computes a
  field and a tool's name that their author is not shown are not
  granted to declare, and no door leaves a lake with a problem among
  its tools' names, where a type declared alone with a relation whose
  link type's door bore another tool's name, or too long a one, was
  applied, and a migration that renamed a relation into one, and the
  lake then refused every declaration, naming the two, to anyone shown
  both (lakelands#893, lakelands#896); a writer's open is such a door
  and an import is not judged by this lake's doors (lakelands#893); 013
  that a trigger,
  a verb and an object set of a bundle name the type the one in force
  stands on (lakelands#893).
- 027 and 024 added: a report's sample keys are the first among the
  records its reader is answered; a collision is named only to one who
  is answered two or more of the records that would take its key and
  reads the new key, in the order of the new key, where a key was shown
  to whoever may see its type; `more` follows from the counts and the
  keys named beside them, where it said only that the report holds more
  than its reader is shown and was never set to one who may see
  everything; a value type's misfits are named of records their reader
  is answered and in fields it reads, and a tightening's report is
  named by what its reader is shown, where a tightening's id named
  misfits its reader was not shown; and a value type's report names a
  type by what its reader is
  described of it, where it had named a type the reader is shown for an
  expression in a member it is not (lakelands#875, lakelands#861).
- Merged at the rewrite (lakelands#817): the rules of 024 and 051, and
  the sections of 013 and 027 on what a principal is shown and told,
  moved here, and the title changed with them; the file keeps its name.
  The base rule of a held record is new: no record stated it.
- Corrected at the rewrite (lakelands#817): 013's "a reviewer without a
  read grant on the record still finds what it is asked to decide, and
  accepts it by its id" and its "decides it as before, what its grants
  hide of a record or a field included". A reviewer that is not
  answered the record is refused as not granted, as 051 said since
  lakelands#772.
- Corrected at the rewrite (lakelands#817): 013's "A reviewer shown
  every type it names decides it ..., what its grants hide of ... a field
  included" and its "a reviewer shown every type an entry names still
  applies the write whole, the part its listing trims included" held of
  a reviewer holding `only` on the type too. That one decides only what
  it is listed whole, as 051 said since lakelands#660.
- Corrected at the rewrite (lakelands#817): 024's "A write to a type
  the reader may not see is listed without its write", 027's "A
  migration of a type its reader may not see is listed without its
  write" and its "A reviewer may accept a migration of a type it may
  not see, by its id", with the report whose `type` is the empty name.
  Such a proposal is not there, and no report has a form with the type
  taken out (lakelands#762).
- Corrected at the rewrite (lakelands#817): 027's "the two schema
  hashes and the report's id stay whole" and "The id names the whole
  report ... the apply is refused or applied as for the whole report".
  The hashes are those of the shown shapes, and a migration's report id
  is made over what its reader is shown; the apply compares the
  caller's own id, and only whether the migration is lossless is decided
  as for the whole report (lakelands#644).
- Corrected at the rewrite (lakelands#817): 051's "takes its
  `restricted` mark, so the holder may not read its new record unless
  it holds that grant" and its "A proposal of each is judged as its
  write". A holder without the grant is refused the create onto a held
  key, as 051 said since lakelands#772, and a `link` and an `unlink` are
  not proposed.
- Corrected at the rewrite (lakelands#817): 013's "Accepting a proposal
  applies it under the proposer's grants as they were when it was
  proposed". It is applied in the proposer's name and not under its
  grant, which is not in the log, as 051 said.
- Corrected at the rewrite (lakelands#817): 024's "A proposal was asked
  when it was made: accepting it applies what was checked then". The
  accept asks its reviewer, as the listing does, and plans the write
  again as the proposer.
- Corrected at the rewrite (lakelands#817): 013's "A principal with the
  review grant lists every proposal and may accept or reject it" and
  "a reviewer only decides whether it happens". A reviewer lists what
  is there to it, a holder of `only` is judged by its own pairs, and
  one without `restricted` neither accepts nor rejects a write that
  reaches a held record.
- Corrected at the rewrite (lakelands#817): 024's "a function, trigger,
  object set or view that names a field the principal may not read is
  not described to it". The definition is not described; a view's type
  is listed with its definition withheld, a holder of `schema` is
  shown the definition, and a proposed bundle's listing, 013's "a view
  with its definition or not at all", leaves such a view out though
  `describe` would list its type.
- Corrected at the rewrite (lakelands#817): 024's "A read at `valid_at`
  ... probes them as a filter on them would" held of a bound its reader
  may not know of too. Such a bound is neither filtered by nor probed
  (lakelands#726).
- Corrected at the rewrite (lakelands#817): 024's "An author who may not
  know of such a field still names it, or sends the relation `describe`
  shows it". A hidden field's name answers as a name no field has, and a
  valid time left out stays as declared (lakelands#726).
- Corrected at the rewrite (lakelands#817): "a view over a view over a
  link type is shown where the type that declares the relation is"; and
  where the relation's target is (lakelands#662).
- Corrected at the rewrite (lakelands#817): 027's "A relation the
  migration removes is listed under `dropped` by its own name, to one
  who sees the type"; and is shown the relation (lakelands#662).
- Corrected at the rewrite (lakelands#817): 027's "so `accept` asks no
  one's rights", true only of completing the shape, and its "the one
  who accepts, who holds the `schema` grant and is shown every trigger,
  set and view", true of those whose type that accepter is shown, as
  024 said of a set since lakelands#877.
- Corrected at the rewrite (lakelands#817): 013's "A link's cardinality
  is judged only where a link is written, and no proposal writes one".
  A proposed call of a verb that sets fields of a link type writes a
  link's own fields; the rule is 013's, and this record points at it.
- Corrected at the rewrite (lakelands#817): "The listing of proposals,
  and the accept and the reject that go by it, alone make more than
  one". Other doors make more than one sight too, among them a write
  that waited while the types moved, whose sight is made a second time
  under its lock, as the rule says; a write door for a principal whose
  grants hide something, first over no declarations and then over the
  head's shape; and a search as of a position, at the head beside its
  own.
- Corrected at the rewrite (lakelands#817): 013's "one who reads every
  field of every type is judged the same there as in force, so an
  owner's listing reads no past". It is shown every type alike, but a
  migration's names are judged for it on the type in force, and a value
  type's migration that renames values is listed to every reader as
  the lake stood before it.
- Corrected at the rewrite (lakelands#817): 051's "A reviewer without
  `restricted` is refused, as not granted, the accept of a write that
  reaches a held record" named the accept alone; `decides` refuses the
  reject as it refuses the accept. And its list of the write doors named
  neither a `link` onto a held link record nor an ingest onto a live
  held key, each refused as an upsert onto the key is, nor a change of
  shape, which names no record and converts a held one with the rest.
- Corrected at the rewrite (lakelands#817): 013's "A retirement's
  refusal names only the record it retires ... so neither is judged". A
  retirement's refusal at the accept is told as its reviewer's listing
  shows the write, as an upsert's and a call's are.
- Corrected at the rewrite (lakelands#817): 024's "A write is governed
  by the principal's write grants" left the verb out: a verb is callable
  only to one who may read what it reads too (lakelands#779;
  `Sight::callable`).
- Corrected at the rewrite (lakelands#817): 013's "one listed without
  its write ... is not granted to decide" held of a record its reviewer
  is not answered and of a holder of `only`; a bundle listed without its
  write to a reviewer without `schema` who holds no `only` is that
  reviewer's to reject.
- Added: a field is asked once a pass of the walk that answers its
  access, where it was asked on every way that led to it and a type of
  forty derived fields that each read the later ones was declared and
  then answered no read; and fields that list each other are read no
  further than each other, all the way round, where a cycle was answered
  by the way the walk came, leaving out the fields it had passed: of two
  fields listing each other, one masked and one its reader writes, the
  first was shown masked and is hidden now; no answer is wider
  (lakelands#926).
- A verb is its caller's only where it reads every field its paths read
  across a reference; what a proposed call reaches and pins gains the
  records it reads across; and a proposal that names a record its
  reviewer is not answered beside its own is not there to that
  reviewer, where it was listed and its accept refused as not granted
  (lakelands#567, lakelands#929).
