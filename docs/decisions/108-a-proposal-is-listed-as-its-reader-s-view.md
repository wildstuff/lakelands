# 108 — A proposal is listed as its reader's view, and an accept decides what its reviewer is shown

These rules are what a listing and an acceptance answer: a proposal listed
as its reader's view, one that names what its reader is not shown left
out, and an accept deciding only what its reviewer is shown.

These rules are one family of the decision in `006 § A call carries one
principal with grants, and the store checks every read and write`, stated
and changed there until lakelands#1768 moved them here word for word.

## What is decided

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
  one who sees the type and is shown the relation (104).
- **A link field's entry**, named `<relation>.<field>`, is answered to
  one shown the relation who may know of that field of the link type,
  under its grants on the link type: a declared link field, and the ends
  `from` and `to`, whose conversion a retarget of the relation names as
  `<relation>.to` (lakelands#1092).
- **A renamed reference field.** For the migrated type, a reference
  field the same migration renames is listed in `references` under its
  new name (027), and the entry is answered to a reader who may know of
  the field in the shape the migration leaves, as a conversion is.
- **`restated`**: an entry is answered as a conversion is: the type's
  own to one who may see the type, a field's to one who may know of the
  field and, where it was renamed, of the name it had. A member is shown
  as `describe` shows the field: one beyond its shape only to one who
  reads the field in `to` and as the type stands (104). An author kept
  from a field restates nothing of it, since the field is put back as
  declared (106).

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

A report's id is made over what its reader is shown (104), so a
preview read trimmed is applied by the id it carried, and whether the
migration is lossless is decided over the whole report: the trim changes
what is read, not what is decided. A proposal and its listing only
tell: a proposal's report is lossless as its proposer is told it, and
the proposal is refused as lossy only by that (below).

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
declared (106), and the accept and the listing answer as for a
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
bundle none of whose declarations it would be shown, and, to a holder
of `only`, a write that would leave a record outside its pairs (109).
The entry keeps its id, its proposer, its times, its state and the
proposer's `why`. A proposal that names a type its reader is not shown
is not listed at all, nor is one on a record its reader is not
answered, held from it or, to a holder of `only`, another's (109).

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
entry as shown. `type` finds what the entry is judged by: a bundle's
verb, trigger or set by the type it is declared on and by the type the
one in force under its name stands on, which the bundle would take
away (lakelands#916).

**A proposed bundle** shows each declaration `describe` would show its
reader once the bundle is in force: a type it could see, with the fields
it may know of; a view with its definition or not at all (above); a
trigger and an object set whose condition it may read; a function and a
value type by `describe`'s rule. A proposed migration of a value type
`describe` does not list to its reader is listed to it without its write
and with `more`, as a type's migration to one not shown the type
(lakelands#1088). A verb is judged by the fields
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

The totals stay whole, and the entry says `more` when anything was
left out. `lossless` is the proposer's: a filed report is lossless as
its proposer was told it, by the records and constraints it was told
of, so it is lossless to every reader. The two schema hashes are those
of the shapes its reader is shown, and the report's id is made over
what it is shown (above). A logged report is lossless, so it names no record that would
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

A link type is shown only with both its ends (104), so a write to a
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
its write or its bundle would apply. Nothing is appended. A proposal on
a record its reviewer is not answered, held or outside a holder's
`only`, is not there to it, its accept and reject answered as an id
never made (below). A reviewer shown every type it names
decides it, what its grants hide of a field included, where the entry is
its to decide (below).

**Why.** An accept is a write to what the proposal names, and its answer
tells what it judged. If it applied, it wrote to a type its reviewer may
not read, or reshaped one. If it was refused, it named the type, a key
of it, a field and its kind, or that a hidden record moved. A listed
entry whose id the accept then calls absent would tell a proposal hidden
by its type from one hidden by a record. To a principal not shown a
type, the type is one that is not declared (109), and a proposal on a
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
migration reshapes or the verb a call calls was declared again since
the proposal, or a type the entry's type rests on was re-keyed since,
the lake as it was proposed against. A type rests on what it is made of
and on the targets of the references it holds: a re-key of one of
those can show a reader, in a key or a reference, an old key that was
the value of a field it may not know of (lakelands#913). A declaration
that keeps the key moves no old key, and asks no past. An entry is not
there when any of them hides a type it names, or when its verb, as the lake
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
- An upsert, a retirement, a call or a migration of a type shown to its
  reader only since a type it rests on was re-keyed is not there: its
  key was made under the old key, the value of a field that reader may
  not know of. One whose type was shown then is listed without a
  reference it could not read then, whose value is that old key, and
  is not its reader's to accept (lakelands#913).
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
that does not exist, word for word, to both; one listed to a holder of
`only` in part, its write withheld or trimmed (below), is not granted
to decide; one listed with
`more` to any other reviewer is not granted to accept, and is its to
reject (lakelands#867); and an accept's refusal is told by the same
answer (below). A bundle that a reviewer
without `schema` who holds no `only` is listed without its write, none
of its declarations shown, is that reviewer's to reject; its accept
takes `schema`.

Accepting a proposal applies it in the proposer's name and not under its
grant, which is not in the log (below): the write was checked whole
under the proposer's grants when it was proposed, and the accept plans
it again as the proposer. It applies the write as it was logged, whole:
trimming what is applied to what the reviewer read would make the
reviewer's grants decide what the proposer wrote, and a write checked
whole would be applied in part. So a reviewer accepts only what it is
listed whole (lakelands#867): the accept asks for `review`, for
`schema` where the ontology changes, and for an entry its listing shows
without `more`. A reviewer listed an entry in part would apply in its
name what it was not shown; it rejects it, since a reject applies
nothing, and its accept is refused as not granted, naming nothing, as
a reviewer's without the grant is. A holder of `only` neither accepts
nor rejects an entry it is listed in part: what is left out there may
be a record the write would make another's, not a field alone (below).
`more` is what the trim left
out: a value type's report, whose id the listing makes again over what
its reader is shown, is whole where nothing of it was left out.

What it costs is said here. A reviewer kept from a field (at `none`)
accepts no upsert whose patch holds it, no call of a verb that sets it,
and no migration of its type, whose `to` it is listed without the
field; masked on it, the upsert and the call. One kept from a type is
listed nothing that names it, which is not there to it. A holder of
`schema` is shown a proposed set's declarations as `describe` shows it
each definition in force, so it accepts a set of a verb that sets a
field it is kept from. And the direct door is wider than the accept: a
holder of `schema` kept from a field declares and migrates the type
itself, the act needing `restricted` too (062), its view carried
back
(`docs/decisions/027-shape-changes-are-declared-or-migrated.md`), and
may not accept the same migration from another. What it authors it has
seen; what another proposes may touch what it has not.

Accepting a change of the ontology needs `schema` and `restricted` as
well as `review`, `restricted` as the act's own door needs it (062).
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
be refused for the set in words that name the type (above). A bundle
takes no place declared since it was proposed: where a type, a value
type, a verb, a trigger, a function or a set it declares was declared
after the proposal, its accept is refused as `declared_since_proposed`,
naming what and which name, to a reviewer it is listed to whole, and it
stays pending. Accepted, it would replace what no one was shown where
it was proposed, as a migration of a type changed since would, and the
accept cannot ask whether its proposer is shown what took the name,
since a proposal keeps no grants. A reviewer that `describe` does not
list what holds the name, a verb that reads a field it may not read
among them, is not listed the bundle, and its accept and its reject
answer as an id that does not exist (lakelands#915). So is a proposal,
of a set or of a migration, whose tool's name was taken since by what
its reviewer is not shown, as one under such a verb's name is
(lakelands#915); a reviewer shown both is refused at the accept, naming
both.

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
