# 027 — A shape change is declared or migrated, and a migration loses nothing

## What is decided

A revision of a type enters the log by `declare` only when no stored
record can stop fitting it: a change of meaning, an added optional field
or relation, a loosened requirement. Every other change of shape — a
field removed or retyped, a field made required, a key changed — goes
through `migrate`, which first answers a preflight report of what would
happen to every stored record, and is applied only by a call that names
that report and only when the report is lossless. A rename is never
inferred from two declarations; only a migration names it.

The conversions a migration may make are a closed table: a value
converts only when converting it back gives the same information. A
number becomes text in its canonical form, a numeric string becomes a
number only when the number is exactly equal, a fraction never becomes a
decimal, an offset is never dropped from an instant, and a value already
in the target form is never rewritten.

## Extended: history converts too, and the key stays

A migration converts every stored record of the type, retired and
restricted ones included: they are history, and a lake whose history no
longer fits its declaration could not be folded again. So the preflight
covers them, and a record among them that would not convert makes the
migration not lossless, refused before anything is appended, never
discovered by the fold after the migration is in the log.

In this release a migration keeps the type's key. Records are converted
where they are and never re-keyed, so a migration whose declaration names
other key fields is refused as not supported, and one that would change
the value of a key field — so that a record's key fields no longer make
its key — reports that record not convertible. Re-keying records is a
later door, not a side effect of a migration.

## Extended: a value type is loosened by declaration and tightened by a preflight

A value type that accepts everything it did (`one_of` gains values or is
removed, a `constraint` or `format` is removed) is declared: no stored
value can stop fitting it. One that may accept less goes through its own
preflight: every value stored under it, in every field of every type that
uses it, history included, is checked against the new declaration as a
write would check it. The change is refused while any value would not
fit, and applied only against the report it was shown. Since a tightening
every stored value fits reshapes nothing, it enters as the ordinary
`declared` event at the next version, and no record is rewritten. A change
of a value type's `kind` or `scale` rewrites stored values (a decimal is
kept at its scale), so it is not made this way; the fields that use it
are migrated one by one.

The preflight looks at stored values, not at the conditions that mention
them. A trigger's `when`, an aging rule's `from` or `to`, or an object
set's `where` that compares with a value the type no longer admits is not
checked; it simply never holds afterwards. For an aging rule the run's own
check applies instead: a rule whose `from` or `to` the type no longer
admits is skipped by `age` and named in its report.

## Extended: a type is re-keyed by a migration

The rule above that a migration keeps the type's key is lifted: a
migration may name other key fields, or change a key field's value. A
record's key is its name everywhere in the lake, so the preflight covers
everything that names the records it moves:

- every record of the type, retired and restricted included, whose key
  the new key fields make differently; two records that would take one
  new key, or a record without a value for a new key field, make the
  migration not lossless;
- every type whose key fields reference a re-keyed type, re-keyed with it
  (link records, keyed by their two ends, are the common case), each
  after the types its own key references (see "a key is made after the
  keys it reads" below);
- every reference value, in any type, that names a moved key.

The moves are listed in the `migrated` event (`rekeyed`, log format
version 2), and the fold moves each record with its fields, version and
per-field provenance unchanged: who wrote a value, and when, stays true of
it. That is why the move is a new member, and a new format version, rather
than a retirement and a new write per record, which would have made every
moved value the migration's. The old key is ended, not retired: a read
before the migration finds it, a read from the migration on does not, and
a later record whose key is the same string is a new record at version 1.
The store keeps an ended key as a retired state at version 0 with no
field. That state is no record: every scan of stored history (a later
migration's preflight, a value type's, the closure of a second re-key)
skips it, so a type that was re-keyed migrates again like any other. A
moved record keeps its object-set memberships at its new key. A migration
that takes every key field away, names a key field that is not required
or of a kind no key is made of, or would re-key a view is refused.

Two things follow outside the lake's own records. A source that writes
the type must send the new keys: its own key mapping (a column map's
`key`, the keys its feed carries) has to follow the migration. A write
whose key fields make another key than the one it names is refused as
invalid, as before; a write that names an old key's bare string and
carries no key field has its key field filled from that string (§2.3),
and is a new record at version 1. And the aging id (`docs/log-format.md` §3) names the record's
key, so a date aged before a re-key and again after it can move a record
a second time, when after its own transition the rule's `from` and `when`
hold for it again.

## Why

The log is the truth (001), so a change of shape must never quietly
make stored records wrong. Separating the two doors makes the dangerous
one visible: `declare` cannot reshape anything, and `migrate` shows its
consequences before it acts. A closed, lossless table means a migration
is reversible in information even when it is not undone, and every
engine converts the same values the same way.

## What it is not

- It is not a ban on removing a field. A removal drops that field's
  values, and the preflight names what it drops before anything is
  applied.
- It does not convert anything the table does not list. A value that
  would not convert makes the migration not lossless, and it is refused.

## Extended: a shape change may be proposed

A declaration or a migration may also be proposed, and it reaches the
ontology only through its acceptance, under the same rule: a change that
reshapes stored records is a migration there too, preflighted when it is
proposed and again when it is accepted
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended).

A value type's tightening may be proposed the same way (lakelands#476):
its preflight runs at the proposal and again at the acceptance, and it
is applied only while every stored value fits and the value type is
still the declaration it was proposed on. The refusal of a narrowed
declaration names both ways: the preflight, for a holder of `schema`,
and the proposal.

## Extended: an annotation is a change of meaning

A revision that changes only a declaration's `annotations` reshapes no
stored record, so `declare` takes it like a relabel. It never needs a
migration, and none is offered. A migration whose `to` changes
annotations together with the shape carries them like any other member
(`docs/decisions/007-schema-hash-covers-shape.md`, extended).

## Extended: a view is never migrated

A migration reshapes the stored records of a type, and a view stores
none: its records are computed on read. So `migrate` refuses any change
that has a view on either side, as invalid, naming the type and the way
to make the change:
- A view that changes (its source, `where`, grouping, measures or
  parameters) is declared again with `declare` or `apply`. Nothing is
  stored, so nothing is reshaped.
- A view that becomes a stored type is declared as one, whatever its
  fields become. It stored nothing, so nothing is reshaped, and the new
  type starts empty.
- A stored type never becomes a view. Its stored records would linger
  beside the records the view computes, and a type's declaration is
  never retired, so `declare` refuses that change too. The view is
  declared under another name.

## Extended: a migration keeps what a view reads

A view reads its sources' shape on every read. A view over a type, as
its source or a merged one, would refuse every read once a migration
took away a field it reads. So the preflight refuses a migration that
drops or renames such a field, naming the view and the field. A field
counts when the view groups by it, reads it in its `where`, or
aggregates it in a measure. A name outside an aggregate is a measure or
a grouping field. This is the rule `describe` uses to decide who may
see a view's definition, and one function holds it for both. The view
is also checked as `declare` would check it against the new shape, so a
grouping field whose kind no key is made of is refused too. A field no
view reads migrates as before.

## Extended: a migration keeps what a declaration reads (lakelands#461)

A view is not the only declaration that reads a type's fields. A field a
migration drops or renames away evaluates as null from then on, so a
set's filter, a trigger's guard or a verb's `requires` over it would
stop holding, or hold where it should not, and nothing would say so. So
the preflight refuses a migration that drops or renames a field read by
a declaration in force, as invalid, naming the declaration and the
field, in the form it uses for a view:
- a derived field of the migrated type: what its expression reads;
- an object set of the type: its `where`;
- a trigger on the type: its `when`, `changed(f)` and `previous(f)`
  included;
- a verb on the type: its `requires`, each field it sets, and what the
  value it sets one from reads (`payload.<name>` is the call's, no
  field);
- another type's derived relation to the type: the field of the type
  each pair matches;
- an object set or a trigger on the link type of one of the type's
  relations: the relation's fields it reads. A relation's fields are its
  link type's, and the type's migration is what changes them, so the
  refusal names the link type as the one the field is read of.

The refusal names the reading declaration only to an author who is
shown it, by the rule `describe` uses for that kind of declaration in
force: a proposer without the `schema` grant is not shown a trigger, a
set or a verb whose expression reads a field it may not know of. Such an
author is told that a declaration reads the field, without its name
("after the migration of `invoice`, a declaration reads `note` of
`invoice`, which the migration drops"). The field it names is one the
author knows of: a field hidden from it was not its to leave out, and its
migration does not take it away (lakelands#475). The refusal stays
`invalid` and can be acted on: the author keeps the field, or asks someone who is
shown the reader (lakelands#495). At an acceptance the refusal goes to
the one who accepts, who holds the `schema` grant and is shown every
declaration.

A view is such a reader too, and it is shown only when its definition
is: `describe` lists a view's type to a principal who may not read all
the definition reads, with the definition withheld. An author who is not
shown the view, by its type or by its definition, is told that a
declaration reads the field it took away, as above. Where the view could
no longer be declared over the migrated shape for another reason (a
field it groups by converted to a kind no key holds, or to another kind
than the merged source it lines up with), no field is gone, and such an
author is told that a declaration over the type could no longer be
declared, without the view and without the reason. An author who is
shown the view keeps the sentences that name it (lakelands#504).

One function gathers these reads, beside the one for a view. The host
declares the reader again first, so that it reads the shape the
migration leaves, then migrates. A field no declaration reads migrates
as before. An aging rule and an index setting are records, not
declarations, and stay outside the rule: each already skips a field its
type no longer has.

## Extended: a removed relation is listed as a removed field is (lakelands#437)

A relation carried by link records that a migration removes keeps its
links as history, and takes them from the head: the walk follows only
declared relations, and the link doors refuse a relation that is gone.
That is what a removed field does to its values, so the preflight says
it the same way: the relation is one entry in the report's `dropped`,
named by the relation, with the number of its live links. A relation
whose links are all unlinked removes nothing from the head and is not
listed. The entry does not make the report lossy, as a removed field
does not: removal is the migration's intent, named before it is applied.

A relation removed and another added is that and nothing more: the old
name is listed with its links, and the new relation starts with none. A
rename of a relation is not inferred, as a field's is not.

## Extended: a shape is its author's view (lakelands#475)

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
  shape, so `accept` asks no one's rights, and a reviewer who may not
  see the field cannot remove it by accepting either.

No refusal on the way names a field its author may not know of. A shape
that names one declares it twice, which is how a taken name is refused.
A rename from one is a rename from no field. A shape that removes a
field a hidden one is built from is refused on the field it removes,
saying only that another field reads it. A masked or a write-only field
is shown to the principal, and stays its to remove.

## Extended: a proposed migration is listed as its reader may know it (lakelands#499)

A proposed migration holds the type's whole shape and the preflight's
report. `proposals` already showed `to` without the fields its reader
may not know of. The rest named them still: a rename's old name, a
dropped field or link field in the report, a re-keyed type the reader
may not see, a reference field of such a type. Each is now shown where
its reader may see the type and may know of the field:

- **`renames`**: a pair whose old name the reader may know of in the
  type as it stands, and whose new name is in `to` as shown.
- **`dropped` and `fields`**: an entry for a field, or a link field
  named `relation.field`, the reader may know of in the type as it
  stands.
- **`rekeyed`**: the types the reader may see.
- **`references`**: the fields the reader may know of, of types it may
  see.

The totals, `lossless`, the two schema hashes and the report's id stay
whole, and the entry says `more` when anything was left out
(`docs/decisions/013-verbs-run-by-declared-risk.md`, extended). A
logged report is lossless, so it names no record that would not convert
or would collide. A migration of a type its reader may not see is
listed without its write.

## Extended: a report is answered as its reader may know it (lakelands#531)

The listing was one of five places that answer a migration's report. The
preview answers one, and a migration that is not lossless is refused
with one, to whoever proposes, applies or accepts it. Those four gave
the report whole: a field hidden from an accepter with its kinds, its
counts and its sample keys; the key of a restricted record to a reader
without the grant; a type the reader may not see among the re-keyed
types, the rewritten references and the colliding keys.

One function trims a report now, and all five places call it. The rule
is the listing's, with two members that only a report that is not
lossless holds, and such a report is never logged:

- **`sample_keys`**: the keys of records the reader is answered. A
  restricted record's key leaves them for a reader without the grant;
  the counts stay whole.
- **`sample_collisions`**: the keys of types the reader may see. A key
  is shown to whoever may see its type.

**`more`.** A report says `more` when it holds more than its reader is
shown, and nothing else of it. A reader told that a migration is not
lossless, and shown no misfit, reads `more`: the reason lies where it
may not look, and one who may see it reads why. The refusal keeps its
kind. A logged and a stored report never say it, so a `write_proposed`
event and a projection row stay what they are.

**The id names the whole report.** A preview read trimmed is applied by
the id it carried, and the apply is refused or applied as for the whole
report: the trim changes what is read, not what is decided.

**An author is a reader like any other.** Its migration is completed
with the fields hidden from it, and the report is made over the whole;
what it may not see, it is not answered, in its own preview too.

**One who may not see the migrated type is answered no name.** A
reviewer may accept a migration of a type it may not see, by its id. If
that accept is refused as not lossless, the report's `type` is the empty
name, which no type's is, every list is empty and `more` is true; the
totals, the hashes and the id stay; and the refusal says that the
migration would lose information, without the type's name. A refusal
that named the type would say what the listing withholds.

A relation the migration removes is listed under `dropped` by its own
name, to one who sees the type.

## Extended: the order of refusals (lakelands#338)

The migration doors refuse in this order, in the frame every write
door shares (016, extended).

**`migrate_apply`**: the `schema` grant (`not_granted`); the preflight:
the type is declared (`not_found`), not every key field is taken away
(`invalid`), every new key field is required (`invalid`, naming it),
the new declaration's own checks, every field a view or another
declaration in force reads is kept (`invalid`, naming the reader), the
type it migrates to reads only fields it has (`invalid`), its history
converts, a narrowed cardinality breaks no live link (`constraint`);
then the report id is the one the preflight answered (`invalid`); then
the migration is lossless (`migration`, with the report); then the set
memberships it changes are within the lake's `max_set_changes`
(`too_large`, naming the set); then each embedding's model is bound
(`unbound`).

**`migrate_value_type_apply`**: the `schema` grant; the value type is
declared, accepts something other than it does, and keeps its `kind`
and `scale` (`invalid` each); its own checks; the report id
(`invalid`); every stored value fits (`invalid`, naming the value
type).

The preflight's own steps answer one kind with the type's name, or
exclude each other (a migration that takes every key field away names
no new one), and are stated, not checked; so are the report id against
the fit of a value type, both `invalid` naming nothing a caller can
tell apart.

## Extended: a key is made after the keys it reads (lakelands#524)

A type keyed by references to two types a re-key moves takes its new keys
from both. Its keys were made when the first of the two had moved, in the
order of the types' names, so a type whose name sorts before the second
kept that one's old keys in its own: a link type, named `sys.link.…`,
before any closure type whose name sorts after it. The fold rewrote the
reference, the key stayed, and a link whose key is not what its ends make
is found from neither end. The preflight had called it lossless.

So the order is the one the keys need, not the one the names have:

- **A type's keys are made after those of every type its key references.**
  The types a re-key can reach are read from the declarations. Each waits
  until every other type its key references has its moves, and is then
  read once, and only if one of them moved anything. The report lists the
  types in that order.
- **Keys made of each other are refused.** A type keyed by a reference to
  itself, or types keyed by each other, have no such order: a key would
  have to be made from a key that is still to move. A re-key that would
  move them is refused as invalid, naming them, and appends nothing. A
  lake holds no record in a ring of two or more types, since the first
  record of each would need a live record of the other; a type keyed by
  itself holds records only when it was re-keyed into that shape, and that
  is what is refused.
- **A renamed reference field is counted.** The report's `references`
  count the values a re-key rewrites. For the migrated type they are read
  under the name its records still hold, so a field the same migration
  renames is listed, under its new name. The entry is answered to a
  reader who may know of the field in the shape the migration leaves, as
  a conversion is.

Where the names' order was already the keys' order, the report keeps its
rows and its id. Where it was not, the rows come in the new order, and a
report made before is refused as stale at the apply.

This repairs no lake. A `migrated` event lists its moves, and the fold
applies exactly those: a lake re-keyed before this keeps what that re-key
left, and a link record whose key is not what its `from` and `to` make
is how such a lake is found.

## Extended: a type its author is not shown is a type that is not declared (lakelands#538)

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
  as its author is shown it, which no longer holds the type
  (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
  extended).
- **To declare the type itself is not granted,** and so is to propose
  it. This is the one door that cannot hide that the name is taken: for
  a free name it creates the type. The refusal names no field and tells
  nothing else, and nothing is written.

The test comes before the author's view is completed and before
anything is appended. An accepter stays as it was: it may accept by its
id a proposal on a type it is not shown, and the accept's answer names
it nothing it may not know of.

## Extended: a declaration in force appends nothing (lakelands#565)

A host states its declarations again at every start. Each statement
appended a `declared` with the next version, so the log grew with every
start, and a declaration's version counted the starts instead of its
changes: one function body declared six times stood at version 6 while
the records it had computed named version 1.

So a declaration that is the one in force is answered `applied` at the
lake's head and appends nothing. Its version stays.

- **In force is judged in the form the lake keeps.** A type is compared
  after the store has put back what its author may not see (024) and has
  completed each computed field's `depends_on`; the type in force is held
  the same way. So a computed field stated with no input and one stated
  with the inputs `describe` shows are the same declaration. The lists
  the type was declared with stay as they were.
- **Every refusal comes first:** the context, the `schema` grant, the
  author's view, the declaration's own checks, the binding of a model or
  an executor. Only a declaration that would have been applied is
  compared, so the rule never answers where a refusal was.
- **A set is compared one declaration at a time, against the ontology in
  force before it.** Its refusals are judged over the whole set, as
  before. What differs is appended in the set's order, and what that
  sets off happens: a recomputation, a set's memberships. A set wholly
  in force is answered at the head.
- **A proposal is recorded as before,** in force or not. When it is
  accepted, the proposal is decided and what is in force at that moment
  is not appended again, so an acceptance may append `write_accepted`
  alone.
- **Every kind:** a value type, a type, a function, a verb, a trigger,
  an object set. A function with a backing and a type with a field
  computed outside the lake are no exception (022).

- **A migration to the type in force** (lakelands#577) is the same case:
  its `to` is the type in force, judged the same way, and it renames
  nothing. `migrate_apply` answers it `applied` at the head and appends
  nothing, after every refusal, a stale report among them: the preflight
  is made and its id compared as before, and answers as before. An
  accepted proposal of one appends its acceptance alone. It used to
  append a `migrated`, raise the version and read every record of the
  type under the writer to change none. A value type's migration to
  itself needs no rule: its preflight refuses it, there being nothing to
  migrate.

**What a declaration in force did besides its event, and no longer
does.** None of it was a promise of this record:
- An engine wrote the head index rows of the type's records again, and
  PostgreSQL built the type's vector indexes again. They came out as they
  were.
- A function computed every live record of every type that names it
  again. One that reads only its inputs changed nothing. One that reads
  a setting or the clock took their values at the declaration, and one
  with a backing called again for an input whose recorded output was a
  failure.
- A type with a field computed outside the lake was settled again: a
  recorded failure was called again, and a field waiting for its backend
  was made.

Each of these now follows a declaration that differs from the one in
force, as the fold decides it from the `declared` event. The log format
does not change, and a log that holds identical declarations folds as
it did.
