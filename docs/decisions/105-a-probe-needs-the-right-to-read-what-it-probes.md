# 105 — A probe needs the right to read what it probes

These rules are what a probe needs: the right to read each field a
filter, an order, an aggregate, a search, a valid time or a walk reads,
and the refusal that follows when its reader does not have it.

These rules are one family of the decision in `006 § A call carries one
principal with grants, and the store checks every read and write`, stated
and changed there until lakelands#1768 moved them here word for word.

## What is decided

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
  how it sees what is withheld, and what a writer writes against. One
  that sees no field, granted nothing of the type but writes, is not
  shown it for reading
  (`docs/decisions/092-reading-is-judged-apart-from-writing.md`).
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
declare (109).

#### A read at a valid time probes the valid-time fields its reader is shown

A read at `valid_at` filters on its type's `valid_from` and `valid_to`,
so it probes them as a filter on them would: `find`, `get`, `search`, a
pattern's start and every type a walk reaches. It filters by the bounds
the reader is shown and probes only those: with both hidden it answers
as for a type with no valid time; with the end hidden, by the start
alone; with the start hidden, by the end alone, a record holding while
it has not ended. A refusal, or a filter by the hidden bound, would tell
the reader that a valid time it is not shown exists. A type in force
with a `valid_to` and no `valid_from` (028) is filtered by nothing, so
its read probes nothing either (lakelands#909). A grant that says
`dates_by_hidden_valid_time` filters by the stored bounds, shown or not,
and probes neither; the bounds stay withheld
(`docs/decisions/092-reading-is-judged-apart-from-writing.md`).

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
the inner one, whose computation probes its own source. The probes of
every source, merged ones included and as deep as the view goes, are
made before the computation reads a record, so what a reader is told
does not turn on the order of the sources: a measure of one source that
fails is told only to a reader every probe admits (lakelands#911). So
every door that computes the view makes them: `find`, `get`, a
pattern's start, and a walk where it computes the view it arrives at.
A read at `valid_at` reads no kept records of a view in whose tree a
valid time is declared, its own or a source's, merged and nested ones
included: a kept fold has no valid time, so whatever its reader is shown
of the view's own valid time, the view is computed at that date, and
every door answers it alike (lakelands#910); the view's own valid time
then keeps its computed records as a type's keeps its stored ones, by
the bounds its reader is shown. Which views those are is
read from the declarations alone, never from what the reader is shown;
a view with none in its tree is read from its kept records at a valid
time as without one, since nothing there is filtered by it. What reads
no source at a valid time probes none: a read without one, which may
read the kept records, and
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
reader (104). So the read is refused as **not granted, on no field**,
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
name is not granted (106). A holder of `schema` is answered the same:
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
relation twice gets, and the type in force does not change (106).

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

## Why

006 states why the store decides a call on grants rather than roles, why
the check sits in the store, and why masking an answer does not protect a
value the question already revealed (`006 § Why`). The reasons particular
to these rules are stated with them, above.

## What it is not

006 states what no rule of grants decides: it is not an off switch, not
authentication, and not partial success (`006 § What it is not`).

Of a probe:

- The rule does not refuse reads of records that merely have such a
  field. A record is still returned with the field masked or absent.
- It does not decide what a writer may write. A write is governed by
  the principal's write grants (012); a verb is held to what it reads
  as well, and what a write's refusal may tell is a rule of its own
  (106, 107).

## How it came to be

- These rules were stated in 006 and changed there from its first version;
  lakelands#1768 moved them here word for word. Their history, oldest
  first, is that record's (`006 § How it came to be`).
