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

A search probes the searchable fields of its type it searches, in every
mode, and the vector field it names. One that names no fields searches
the searchable fields its principal reads (lakelands#662): a field it
sees masked or may not know of is not searched, so it neither refuses
nor ranks the search, and where every searchable field is hidden from
it the search answers no hits. A vector embedded from text encodes that
text, so a vector or hybrid search also probes every field its vector
is embedded from, even when the principal may read the vector itself.
A hybrid search that one of these refuses is refused whole, never
answered from its other half. This tightened in 0.5.0 (lakelands#242).

A lexical or hybrid search may name the searchable fields it matches in
(`fields`). It then probes those fields instead of the searchable ones
it reads, so it may match in a field its reader may not read only by
naming it, and is then refused. Only the text half narrows: the vector of
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

**A setting the store reads is judged the same way (lakelands#601).**
`sort_index.<type>`, `vector_index.<type>` and `view_kept.<type>` name
a type, its fields and, for a view, what its measures read, and the
judge's sentences say whether each is declared, of which kind, and why
a view cannot be kept. An applied write tells as much as a sentence
does, and it acts: an index is built, a view is kept. So the judge
comes after the writer's grant on `sys.setting`, and it is handed the
type as `describe_type` answers it to the writer, the one function
that already decides what a principal is shown:

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

**A write asks first whether its writer is shown the type
(lakelands#607).** A principal shown nothing of a type was told by its
writes what `describe` kept from it: an `upsert` was refused field by
field (`` `vault` has no field `code` ``), a `retire` and a verb's call
were refused as not granted, a write to a view was told it is a view,
and each of these is another answer than the one a name that is not
declared gets. So every door that writes a record asks the question the
link doors ask (`docs/decisions/006-grants-not-roles.md`, extended)
before anything else is judged:

- **`upsert`, `retire`, `propose` of either and `ingest`** answer one
  not shown the type as for a type that is not declared, before the
  patch, the key's form, the view's refusal, the grants and the version
  are looked at. `ingest` asks its own grant first, as before: one
  without it is refused whatever it names.
- **A verb on a type its caller is not shown is a verb that is not
  declared**, for `call` and for `propose`, before its scope is judged
  in a sentence that names the type. A caller that is shown the type is
  told what it may not do there: a verb it may not call stays refused
  as not granted.
- **A retry is answered first**, as at the link doors: a write made
  under an idempotency key is answered that write to the principal that
  made it, whatever it is shown since.
- **It is asked of the type the caller names.** `link` and `unlink`
  name the owner and are asked of it; the link record they write is then
  judged by its grants, so a caller shown the owner and kept from the
  link type by its own grant is refused by that grant, as before. A verb
  is asked of its type once, at the call; the record it writes is not
  asked again.
- **A proposal was asked when it was made**: accepting it applies what
  was checked then, and aging writes as the store.

The question is the two tests that decide whether a type is described,
some access to the type or to one of its fields and the same of every
type it is made of, as one function that the description calls first.
Asking the description itself was measured and refused: it copies the
lake's declarations, 209 µs a write in a lake of two hundred types
against a write of 75 µs, where the two tests take nanoseconds.

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

## Extended: a report's sample keys are of records its reader is answered (lakelands#531)

A migration's report names records by their keys: the first that do not
convert, the first new keys that collide. A key is a name of a record,
and a reader without the `restricted` grant is answered a restricted
record by no read. So a sample key of such a record is not answered to
it either, in a preview and in a refusal alike; the count beside it
stays whole, and the report says `more`
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
extended).

## Extended: a computed field is read as what it is computed from, listed or not (lakelands#540)

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
- **An expression is read as it is checked** (lakelands#652). A derived
  expression is checked as part of a write, where `previous` and
  `changed` have meaning, and what it reads is told the same way: a
  field built with `previous(b)` or `changed(b)` is built from `b`. Told
  as a read outside a write, such an expression had no reads, and the
  field was read whole by a principal kept from `b`. An expression whose
  reads cannot be told is completed with every other field beside it, of
  its type or of its relation: a field whose reads are unknown is
  closed, never open, so no function that one parse accepts and another
  refuses can open a field again.

An embedding names what it is made from in `embed.from`, which has no
unlisted form; it is untouched.

Why not follow the expression where rights are asked: a field's access
is asked for every field of every record a read answers, a function's
inputs are on another declaration, and the list `declare` checks and
`describe` shows would then say less than the rule that reads it.

## Extended: a declaration reads only what its author may read, and a reaction is named to one who reads its condition (lakelands#538)

A principal with the `schema` grant that was kept from a field could
still read it, record by record. It declared an object set or a trigger
whose condition named the field, and the answer of a write named the
set a record joined and the trigger that fired. It needed no declaration
of its own where another's condition read the field: `describe` shows
one who holds `schema` every definition, and a write's answer named
every reaction `describe` lists to its caller. The same declaration also
told whether a name was a field's: one over a field the type has was
taken, one over a name it has not was refused.

Two rules close it.

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
  still set such a field: a call is held to its caller's right to write
  each field the verb sets, so the declaration grants nothing.
- **What a type's shape names stands as declared.** A valid time and a
  relation's `via` are named by the shape, not read by it. An author who
  may not know of such a field still names it, or sends the relation
  `describe` shows it, and declares its type again or migrates it as
  before: the reads are held to its view, the shape is not. A key field
  is different: an author who may not know of one is not shown the type
  (006, lakelands#715), so its declaration of the type is not granted
  and a migration of it finds no type.
- **Only what its author adds or changes is its author's.** A type that
  holds another's derived field over a field its author sees masked is
  declared again as it stands; a field of the author's own over the same
  field is refused.
- **`add_field` adds to its author's view of the type.** It built the
  new shape from the whole type before, so a field derived from a hidden
  one went in through it, and an author kept from any field could add
  none.

**Not at the accept.** An accepter did not write the declaration, and a
refusal there would name a field to one who may not know of it. The
author is held at the proposal; a proposal made before a lake ran this
is accepted as it was proposed.

**A write's answer names a reaction to one who reads what its condition
reads.** `fired`, `joined` and `left` name a trigger or an object set
only where the one answered is listed it by `describe` and reads every
field its condition names. Being shown a definition is not reading what
it reads. So a condition another principal declared, or one in force
from before, tells its reader nothing by name.

**What stays.** A principal with `schema` is shown every definition, and
a write answers `through`, the position its consequences reach. One who
is shown a condition and sees that a write reached further than itself
can infer that the condition held, where nothing else explains it. The
lake no longer answers a name or a value; it cannot hide that something
reacted from one who must read its own write's consequences. So `schema`
is not handed to a principal that is kept from a field or a type
(`docs/hosting.md`).

A declaration in force that reads a field its declarer may not read
stays in force. It is held when it is declared again, and until then the
second rule covers what it tells.

## Extended: a field is the same field under another name (lakelands#527)

The right to read a field is asked by the field's name, and a migration
can change the name. The rule above holds through it: a field a
principal may not read is not read under the name a rename gives it, nor
under the name it had in a read as of before. How a grant holds a field
through its renames is in `docs/decisions/006-grants-not-roles.md`,
extended.

## Extended: a value type's report is shown as its listing is (lakelands#538)

The preview of a tightened value type listed every type and field that
uses it, with its counts, and the refused apply named the first values
that would not fit, to a caller that was shown neither the type nor the
field. A proposal's listing was held already: a use says something of a
field's values. Now the preview and the refused apply are held the same
way. A use and a misfit are shown where their caller is shown the type
and reads the field; the totals and the report's id, which names the
whole report, stay.

## Extended: a named set is read by its filter's right (lakelands#552)

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

## Extended: a field's declaration is shown by one list, wherever it is answered (lakelands#554)

`describe` showed a field its reader may know of and not read by its
shape alone, and a migration's report, a proposed migration's `to` and a
proposed declaration's types did not: a reader with a field masked was
handed its label, its description, its annotations, whether it is
searchable and the expression it is computed by, in the listing of a
proposal and in a report's `restated`. The `to` of a listing stood so
before `restated` existed.

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

## Extended: a schema hash is that of the shape its reader is shown (lakelands#644)

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
(006), so a field renamed since and hidden under its new name is in
neither hash. `more` is decided
before the hashes are set, so it says nothing the hash would.

**The id is made over what is shown.** A report's id names the report
and the head it was made at, and it is computed over the report as its
reader is shown it, so the reader can compute it from what it holds and
learns nothing from it. `migrate_apply` compares the caller's own id;
an accept compares no id, and a listed proposal's id is made at the
head before the proposal.

**What stays whole.** The stored report, the accept's comparison (a
type changed since the proposal, in a hidden field too, refuses it),
the log's `declared` and `migrated` events and the feed hold the whole
hash. One version of a type may therefore have several hashes, one per
shown shape, and a hash compares only between readers shown the same
shape and across one reader's answers over time; the version moves on
every change, a hidden one included, which a description already does
without the hash.

The rest of a type's description is computed over the same shown shape
(lakelands#662). Whether the type is searchable and whether it is
sourced are said of the fields its reader is described: a field it sees
masked is described without `searchable` and is searchable to it no
more, and one it may not know of says nothing. A relation is described
only to a reader shown both its ends, the field that carries it where
it is carried by a reference field, and, for a derived relation, every
field its match pairs
(`docs/decisions/006-grants-not-roles.md`, extended). What remains
says a hidden thing exists, and remains on purpose:

- the version, which moves on every change, as above;
- the key, the valid times and a relation's `via`, which are the shape
  and are given of a masked field too. A key field the reader may not
  know of hides the type instead (006, lakelands#715), since the key
  would give its value. A valid time naming such a field is not named
  (lakelands#726): `describe`, `describe_type`, a listed proposal and a
  migration's report give the type without that bound. A read at a
  `valid_at` filters by the bounds the reader is shown and probes only
  those: with both hidden it answers as for a type with no valid time,
  with one hidden as for a type declared with the other alone, word for
  word. A refusal, or a filter by the hidden bound, would tell the reader
  that a valid time it is not shown exists;
- a reference field's target, which is its type: hiding it would make a
  field's access depend on another type at every read. A host that
  hides a type grants `none` on the reference fields to it;
- `may.retire`, which says every field is writable.

## Extended: a relation carried by a field is walked by its reader only (lakelands#727)

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
`schema` who may not know of the field now sends a type whose target it
is shown, so a relation it sends under the hidden relation's name is
refused as one declared twice, in the words any type declaring a
relation twice gets, and the type in force does not change (027,
extended).

This is a change for a host: a reader held from such a field, which
walked the relation before, is now told it is not there.
