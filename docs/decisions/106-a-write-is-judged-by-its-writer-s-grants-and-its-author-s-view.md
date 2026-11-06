# 106 — A write is judged by its writer's grants and its author's view

These rules are what a write is judged by: its writer shown the type
before any grant is checked, the grants before the data, what the refusal
may tell, and the author's view of the declaration or the migration the
write carries.

These rules are one family of the decision in `006 § A call carries one
principal with grants, and the store checks every read and write`, stated
and changed there until lakelands#1768 moved them here word for word.

## What is decided

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
`vector_index.<type>`, `lookup_index.<type>` and `view_kept.<type>` name a type, its fields
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
- A setting that names a type's fields reaches the judge only from a
  writer that reads the type whole (below), so no field it names is one
  the writer is not shown, masked or write-only.
- A view shown without its definition cannot be kept, and the refusal
  says no more: the reason names a measure, a call and a field of the
  source, and is said to one shown the view as declared, as `kept` says
  it.
- The judge reads the value as its writer writes it. A setting that
  names a type's fields is told, and may be written, retired or
  proposed, only by a writer that reads the type whole; any other is
  refused it as not granted before anything of the record is judged
  (`docs/decisions/063-every-setting-is-in-one-list.md`), so no standing
  name the writer is not shown is judged, dropped or counted in what
  its write answers.

The judge once read the value the write leaves, standing names too: a
principal extended or patched a setting only while shown everything it
named. That left the setting to one who sees more, and the refusal told
the principal that the setting names something it is not shown. It was
then judged as written, over a record told narrowed, whose version still
counted the names its writer was not told; the setting is now told and
written whole or not at all.

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
  asks its reviewer, as the listing does (108), and writes as the
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
- a field hidden from the author (`Sight::field_access` answering
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
know of (104), kept the same way; one the author sends names a type it
is not shown, and is refused as one that is not declared, so naming its
target in force answers as naming any other hidden type or no type, and
the author learns nothing of the target. A masked or a write-only field
is shown to the principal, and stays its to remove.

What a type's shape names stands as declared only where it stands. A
valid time left out where the one in force names a field outside the
author's view, which `describe` does not name to it (109), stays as
declared: it is left out of the author check and checked whole at apply,
so no new member can name that field through it, and it is put back with
the hidden fields. One the author sends, the one in force restated too,
is judged by its name first, as on a type with no valid time: a name no
field has, or a field the author may not know of, answers word for word
as there. A field the author may name is refused at the type: the bound
is taken, as a name a hidden field holds is, and it stands. A key and a
relation's `via` never name such a field: a type keyed by one is not
shown (104), nor a relation carried by one, nor is a reference to a
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
  of one is not shown the type (104), so its declaration of the type
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
  holder of `schema` is shown every function, and is kept all the same
  from declaring again one that computes a field of a type it is not
  shown: a function stands on the types whose fields it computes, and
  what `schema` may change follows its sight, where what it is told
  stays whole (lakelands#917). An author that is shown the one in force
  moves it to another type, or gives it a new body, by declaring it
  again.
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
- **A link type its author is not shown takes no field from it.** A
  relation its author is shown keeps, declared again, the link fields
  it may not know of. But where the link type itself is one it is not
  shown, a field it sends there would be told against fields it may not
  know of: one named like one of them refused as declared twice, one
  named otherwise added beside them. So such a declaration is not
  granted, naming nothing, as declaring that type is; sent without
  fields, the relation keeps its link type's (lakelands#965).
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
shown the reader. What the refusal names at an acceptance is (108).

A view is such a reader too, and it is shown only when its definition
is (104). An author who is not shown the view, by its type or by its
definition, is told that a declaration reads the field it took away, as
above. Where the view could no longer be declared over the migrated
shape for another reason (a field it groups by converted to a kind no
key holds, or to another kind than the merged source it lines up with),
no field is gone, and such an author is told that a declaration over
the type could no longer be declared, without the view and without the
reason. An author who is shown the view keeps the sentences that name
it.

### A grant may admit every record to read and only some to write

A principal's grant on a type may hold `writes_only`: pairs of a field
and a value, valid as those of `only` are, a field the type declares, of
kind `text` or `reference`, that the holder reads, with a text value.
Its holder reads what its other grants give it, every record where they
give every record, and writes only the records whose fields hold every
pair (lakelands#1490).

- **Judged before and after.** A write is judged against the record at
  its key, live or retired, and against the record it leaves: both must
  hold the pairs. A change, a handover to other values and a retirement
  of another's record are refused, and so is a create that does not
  leave the pairs: a value the write gives, a field's `initial` or an
  `authority: writer` field that names the holder's own writer counts.
  A retired record of another blocks a create at its key, so a holder
  cannot take another's key over by its retirement. Every write door is
  judged so: `upsert`, `retire`, a verb, `ingest`, a proposed write and
  its accept, and a change to records, whose plan names each record the
  holder may not write as refused and whose `enact` is then refused
  whole. The refusal is `not_granted` and names no field: the holder
  reads the record, so the refusal tells it nothing it could not read.
- **A grant that cannot be applied** is refused as invalid at the doors
  that write the type, naming the type, as `writes_only_unappliable`.
  It changes no read: a grant that says nothing about reading is no
  reason to hide what the other grants show.
- **Links.** A link is its holder's to make by one end it holds: a
  record of an end type that carries the holder's `writes_only` and
  holds its pairs. A holder links its own record to one it may only read.
  An end of a type carrying no `writes_only` is held by no grant, not
  even by a grant that writes the type: the pairs are what make a record
  the holder's, and a type without them names no record as its own. A
  link with no end held is refused, not opened, so a holder of
  `writes_only` on the link type alone makes no link. Both ends must be
  its own where the relation is to-one at the other end, since the link
  then takes that end's one place. Where the relation is to-many at the
  other end, the holder's one end suffices: no declaration accepted can
  name a relation in a computed field's expression or `depends_on`, so a
  link changes no answer of the other end but through its one place. The
  link record itself must hold the pairs the holder's `writes_only` on
  the link type names, at its create, at an `unlink` and at any other
  write to it. A holder of `writes_only` on an
  end type who holds none on the link type makes no link of it: the
  case is not covered, so it is refused, not opened.
- **Proposals.** A proposal is judged as the direct write is, and its
  pairs pin it as those of `only` do: a write that may move them, a verb
  that assigns a field they name, is not proposed, and the accept is
  refused once the record moved since. A records proposal carries the
  proposer's `writes_only` pairs into its `only`, so a change accepted
  later writes only the records its proposer could; a pair that would
  contradict one of `only` is refused. A reviewer holding `writes_only`
  accepts only what its pairs admit, at the record at the key and in the
  write it leaves, and rejects only that too: deciding is writing the
  outcome, and a reviewer is no wider when it says no. It decides no
  schema proposal, as a holder of `only` decides none.
- **Schema.** With `schema` a holder may propose a declaration or a
  migration; `declare`, `apply`, `migrate` and the other direct schema
  doors stay refused, as does an aging rule on the type: a shape is the
  whole type's, and a change to it moves records the holder may not
  write.
- **`describe`** tells the holder its pairs as `may.writes_only`, offers
  `upsert` only where a create can hold them, a relation only where its
  link type's grant can be held, and no write, link or verb that would
  be refused.
- **With `only`.** A grant may hold both: its holder reads what `only`
  admits and writes only what both admit.

**Why.** A team often reads the whole lake and is answerable for a part
of it. Before, the host had two choices, each wrong: give `only` and
hide the rest from a reader who needs it, or give the type and let the
reader write records that are not its own. Judging the record before and
after is what makes the pairs a boundary: a check of the after alone
lets a write take another's record over by giving it the holder's
values, and one of the before alone lets the holder hand its record out
of its own reach. Where a rule does not cover a case, the case is
refused, so a host never learns of a gap by a write it did not mean to
allow.

## Why

006 states why the store decides a call on grants rather than roles, why
the check sits in the store, and why masking an answer does not protect a
value the question already revealed (`006 § Why`). The reasons particular
to these rules are stated with them, above.

## What it is not

006 states what no rule of grants decides: it is not an off switch, not
authentication, and not partial success (`006 § What it is not`).

Of `writes_only`:

- It is not taken from `Principal.id` implicitly: a host that means a
  principal's own records writes the pair, or names an
  `authority: writer` field with that principal's writer.
- It does not change `only`: `only` still decides what is read, and a
  grant holding both writes only what both admit.
- It does not change a read, nor counts as one: a grant whose reads
  cover every record still does with it.

## How it came to be

- These rules were stated in 006 and changed there from its first version;
  lakelands#1768 moved them here word for word. Their history, oldest
  first, is that record's (`006 § How it came to be`).
