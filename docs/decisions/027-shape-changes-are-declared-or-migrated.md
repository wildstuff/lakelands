# 027 — A shape change is declared or migrated, and a migration loses nothing

## What is decided

### A shape change is declared where no stored record can stop fitting it, and migrated otherwise

A revision of a type enters the log by `declare` only when no stored
record can stop fitting it: a change of meaning, an added optional field
or relation, a loosened requirement. Every other change of shape — a
field removed or retyped, a field made required, a key changed — goes
through `migrate`, which first answers a preflight report of what would
happen to every stored record, and is applied only by a call that names
that report and only when the report is lossless. A rename is never
inferred from two declarations; only a migration names it.

### An annotation is a change of meaning

A revision that changes only a declaration's `annotations`, a value
type's among them, reshapes no stored record, so `declare` takes it
like a relabel. It never needs a
migration, and none is offered. A migration whose `to` changes
annotations together with the shape carries them like any other member
(`docs/decisions/007-schema-hash-covers-shape.md`).

### Marking a type written once is a change of meaning

A revision that only marks a type `insert_only`, or takes the mark off,
changes no stored record, so `declare` takes it as it takes a relabel:
the type's version moves, its schema hash does not, and no migration is
asked. From that position its source writes each key once
(`docs/decisions/016-two-write-doors-by-origin.md`, lakelands#1044).

### A migration converts by a closed table, and only where converting back gives the same information

The conversions a migration may make are a closed table: a value
converts only when converting it back gives the same information. A
number becomes text in its canonical form, a numeric string becomes a
number only when the number is exactly equal, a fraction never becomes a
decimal, an offset is never dropped from an instant, and a value already
in the target form is never rewritten.

### A migration converts history too

A migration converts every stored record of the type, retired and
restricted ones included: they are history, and a lake whose history no
longer fits its declaration could not be folded again. So the preflight
covers them, and a record among them that would not convert makes the
migration not lossless, refused before anything is appended, never
discovered by the fold after the migration is in the log.

### A shape is its author's view

A migration's `to`, and a type declared again, is read as its author's
view of the type: a field or a relation hidden from the author is put
back as declared, the report is made on the completed shape, and a type
its author is not shown is to it a type that is not declared
(`006 § A declaration or a migration is its author's view`).

### The report says every difference between the declaration in force and `to`

A host built a migration's `to` from a reduced view of its type: the
field named, its type, nothing else. The migration was applied and its
report read `lossless: true` with nothing listed, and afterwards the
field was no longer `sensitive`, no longer searchable, and had lost its
label and annotations, as had the type its own. No value was handed out
by it: a reader without the grant is not told a field exists once the
mark is gone, where it was told of a masked one before. What went was
the mask's notice, the search and the meaning, and no answer said so.
The walk that followed found the same silence for most of a declaration:
a field's authority, expression, function, `depends_on` and embedding,
the type's key where no record moves, its valid-time fields, every
member of a relation, an optional field or a relation added, and a
relation removed that held no live link.

**`to` stays whole.** A migration's `to` is a declaration, and a
declaration is what it says (028). A `to` that kept what it leaves out
would be partial, and could then take no mark away, since a mark left
out and a mark set false are one form. So the remedy is the signal:
**every difference between the declaration in force and `to` is in the
report.**

- **`fields`** says a field's type that changes and a `required` that
  tightens; **`dropped`** a removed field, and every removed relation,
  with the number of its live links; **`relations`** a renamed
  relation.
- **`restated`** says every other member, one entry each:
  `{field, member, was, to}`. It is the comparison of the two
  declarations as they serialise, so the store keeps no list of
  members, and one added to a declaration later is reported without a
  line of code. `field` is left out for the type's own members, is a
  field's name, a relation's name, or `relation.field` for a link
  field; a field is compared with the one it takes its values from, so
  a renamed field reads `name` from its old name to its new one. A
  member left out reads null.
- **A field or a relation `to` adds** is restated from null, in each
  member it states otherwise than a bare declaration does, its name
  first: a reviewer reads `sensitive`, or its absence, at the proposal.
- **Both shapes are completed first** as one in force is (006): a
  `depends_on` the author leaves to the store restates nothing.
- **`lossless` keeps its meaning.** It speaks of stored values, and a
  migration that only restates is lossless. Taking `sensitive` away is
  a legitimate change and is not refused; the report is where it is said.

**What a reader is answered of it.** An entry is answered as a
conversion is, and a report says `more` where an entry was left out of
it or a count is larger than the keys named beside it
(`006 § A report is answered as its reader may know it`).

**The log.** A proposed migration is logged with its report, so with
`restated`, which is left out when it is empty
(`docs/log-format.md`, Appendix A).

**An accept restates what the proposal's report said, no more.** An
accept refuses a migration proposed on another shape by the schema
hash, which covers a field's name, type and `required` alone. A mark
set while the proposal waited, `sensitive` on a field the migration
names without it, would be taken away by the accept and shown to no
one. So the accept compares what the migration would restate on the
type as it stands with what its logged report restates, and refuses a
difference as it refuses another shape: "changed since this migration
was proposed; propose it again".

A report's id is computed over the report as its reader is shown it,
`restated` with it
(`006 § A description, a schema hash and a report's id are those of the shape their reader is shown`).

### A removed relation is listed as a removed field is

A relation carried by link records that a migration removes has its
live links retired by the migration, in its own append: one
`record_retired` a link, caused by the `migrated`, with what a
retirement sets off, as a purge retires what it takes (035). The
history keeps them and a read as of before answers them; at the head no
door does. That is what a removed field does to its values, so the
preflight says it the same way: the relation is one entry in the
report's `dropped`, named by the relation, with the number of live
links its apply retires, held ones among them, so the count is whole to
every author. A relation whose links are all unlinked removes nothing
from the head; it is listed with none, as a field no record holds is,
and so is a relation that is not carried by link records. The entry
does not make the report lossy, as a removed field does not: removal is
the migration's intent, named before it is applied. No limit bounds the
append, as none bounds a purge's.

A relation removed and another added is that and nothing more: the old
name is listed with its links, and the new relation starts with none. A
rename of a relation is not inferred, as a field's is not. A relation
declared again later under a removed one's name starts with none too.

### A type is re-keyed by a migration

A migration may name other key fields, or change a key field's value. A
record's key is its name everywhere in the lake, so the preflight covers
everything that names the records it moves:

- every record of the type, retired and restricted included, whose key
  the new key fields make differently; two records that would take one
  new key, or a record without a value for a new key field, make the
  migration not lossless;
- every type whose key fields reference a re-keyed type, re-keyed with it
  (link records, keyed by their two ends, are the common case), each
  after the types its own key references (below);
- every reference value, in any type, that names a moved key.

The moves are listed in the `migrated` event (`rekeyed`), and the fold
moves each record with its fields, version and per-field provenance
unchanged: who wrote a value, and when, stays true of
it. That is why the move is a member of the event, rather
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
invalid; a write that names an old key's bare string and carries no key
field has its key field filled from that string (`docs/log-format.md`
§2.3), and is a new record at version 1. And the aging id (§3 there)
names the record's key, so a date aged before a re-key and again after
it can move a record a second time, when after its own transition the
rule's `from` and `when` hold for it again.

### A key is made after the keys it reads

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
  a conversion is
  (`006 § A report is answered as its reader may know it`).

The order repairs no lake. A `migrated` event lists its moves, and the
fold applies exactly those: a lake re-keyed while keys were made in the
order of the names keeps what that re-key left, and a link record whose
key is not what its `from` and `to` make is how such a lake is found.

### A migration keeps what a view or another declaration in force reads

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
view reads migrates.

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

One refusal of this kind reads no field. A migration that removes a
relation carried by link records is refused while a verb in force is
declared on the relation's link type, whatever the verb reads: it would
be left the one door to a link type that takes no link (035).
The refusal has a case of its own, names the relation and the link
type, and names the verb as below: to an author `describe` shows it to,
one who may call it. A trigger, a view and an object set of the link
type do not refuse the removal. A caller who is not shown the verb is
answered the same case without the verb: that a declaration is declared
on the link type
(`006 § Some things are still told, on purpose`).

So is a migration that removes such a relation while a relation or a
reference field that stands after it refers to the relation's link
type, of another type or of its own: it would name the links of no
relation (035). The refusal has a case of its own, names the relation
and the link type, and names the relation or the field that refers,
placed at its type, to an author shown it. One who is not, kept from
its type or from the link type, is answered the same case without it:
that a declaration refers to the link type. Removed in the same
migration, the other relation refuses nothing.

A migration that re-keys the type and removes such a relation is
refused, naming the relation: the re-key moves the links the removal
retires. The host makes them two migrations, as for a rename.

The refusal names the reading declaration only to an author who is
shown it, by the rule `describe` uses for that kind of declaration in
force; another is told that a declaration reads the field, without its
name
(`006 § A declaration or a migration is its author's view`).

One function gathers these reads, beside the one for a view. The host
declares the reader again first, so that it reads the shape the
migration leaves, then migrates. A field no declaration reads
migrates. An aging rule and an index setting are records, not
declarations, and stay outside the rule: each already skips a field its
type no longer has.

### A view is never migrated

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

### A value type is loosened by declaration and tightened by a preflight

A value type that accepts everything it did (`one_of` gains values or is
removed, a `constraint` or `format` is removed) is declared: no stored
value can stop fitting it. One that may accept less goes through its own
preflight: every value stored under it, in every field of every type that
uses it, retired and restricted records included (not earlier versions),
is checked against the new declaration as a
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
admits is skipped by `age` and named in its report. A migration that
renames values lists, in its report, what may name one (below).

### A value type's migration renames values

A host that wants `open` to read `active` everywhere could only widen the
value type, write every record, and tighten it again: one append per
record, each a new version the host wrote, and a retired record still at
the old value either held the tightening back or had to be brought back
to life to be changed. Renaming a value is a change of the shape, not of
the records, so it is made where shape changes are made.

- **The migration takes `renames`,** pairs of an old and a new value of
  a text value type, beside the new declaration. Each old value is one
  the declaration in force accepts, each new one one the new declaration
  accepts, and no value is renamed twice or to itself. Every pair maps a
  value as it stood before the migration, so a chain and a swap need no
  order; two old values may take one new one, which the lake cannot undo,
  as it cannot bring back a removed field. Numbers and instants are not
  renamed: each would need its one written form under the declaration,
  and no host has asked for it. Without renames the migration is the
  tightening above, byte for byte.
- **The preflight counts through the renames.** Each stored value is
  renamed first and then checked; the report counts, per field, how many
  values each pair rewrites. It refuses a rename of a value a key field
  holds: the record would keep a key its fields no longer make, and the
  next write of the new value would make a second record; a re-key goes
  through the type's migration first. It refuses a field the lake
  computes from an expression that holds an old value, which would bring
  the old value back at its next computation. A field computed outside
  the lake is not renamed, since the lake does not write what a backing
  answers: an old value there is a misfit, and the way through is to
  widen the value type, change the function, and rename then.
- **It is one event of its own kind, `value_type_migrated`,** not a
  `declared`, so that a consumer of the feed sees a change of stored
  values for what it is, under each type it rewrites. The fold applies it
  as a step, as it applies a type's migration: every record of every type
  that uses the value type, retired and restricted ones included, takes
  the new value at the event's position, with its version, its flags and
  each field's provenance as they were. What reads a renamed value, a
  derived field, a field computed by a function, an embedding, is computed
  again as of the event, and what it owes a backing is owed to it: the
  call that carries a renamed value, and no call that is as it was, so a
  recorded failure stays its field's answer until `recompute` names it
  (`docs/decisions/022-functions-are-declarations.md`). A field's
  `initial` that names an old value is declared again by the lake with the
  new one, after the event, as one set: a type's own field cannot name a
  value before the value type in force accepts it. Such a declaration of a
  type with a field computed outside the lake settles that type's records
  again, as every declaration of such a type does. A page of a fold may
  end between the event and those declarations, and nothing differs: the
  fold never reads `initial`, only a write that makes a record does. The
  sets decide their members again in the same append, under the lake's
  `max_set_changes`, over the records as the step and that computation
  leave them, with the new declaration in force.
- **What may name an old value is listed, not changed.** A set's
  `where`, a trigger's `when`, a verb, a view, an expression of a
  function that holds an old value as a literal, and an aging rule on a
  type that uses the value type that moves from or to one, are listed
  in the report by their kind and name, to a reader shown them; another
  is told that there may be one
  (`006 § A report is answered as its reader may know it`).
  A literal is listed wherever it stands, so the list may name more
  than reads the field: it says what may name one. Grants' `only` pairs
  and filters a host or a source holds are not in the lake's
  declarations and cannot be listed. Changing them is the author's, as
  the rule above says of a value no longer admitted.
- **Renames write every field that uses the value type,** so a caller
  not shown one of them is not granted the preflight, the apply or the
  proposal, and nothing is named to it
  (`006 § A declaration or a migration is its author's view`).
  A proposal is accepted only over the fields its report named: one
  that came to use the value type since would be rewritten unseen by
  the proposer and the reviewer.

### A shape change may be proposed

A declaration or a migration may also be proposed, and it reaches the
ontology only through its acceptance, under the same rule: a change that
reshapes stored records is a migration there too, preflighted when it is
proposed and again when it is accepted
(`docs/decisions/013-verbs-run-by-declared-risk.md`). Its accept is a
reviewer's only where the reviewer is listed it whole: one kept from a
field of the type, listed `to` without it, may migrate the type itself
through the direct door and may not accept the same migration from
another (lakelands#867,
`006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows`).

A value type's tightening may be proposed the same way:
its preflight runs at the proposal and again at the acceptance, and it
is applied only while every stored value fits and the value type is
still the declaration it was proposed on. The refusal of a narrowed
declaration names both ways: the preflight, for a holder of `schema`,
and the proposal.

A proposed migration is listed to each reader as that reader may know
it, its `to` and its report alike
(`006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there`).

### A declaration in force appends nothing

A host states its declarations again at every start. Each statement
appended a `declared` with the next version, so the log grew with every
start, and a declaration's version counted the starts instead of its
changes: one function body declared six times stood at version 6 while
the records it had computed named version 1.

So a declaration that is the one in force is answered `applied` at the
lake's head and appends nothing. Its version stays: a version counts a
declaration's changes, and the times `recompute` appended it again (022).

- **In force is judged in the form the lake keeps.** A type is compared
  after the store has put back what its author may not see (006) and has
  completed each computed field's `depends_on`; the type in force is held
  the same way. So a computed field stated with no input and one stated
  with the inputs `describe` shows are the same declaration. The lists
  the type was declared with stay as they were. A grouped view is
  compared by the key its records carry, its `group_by`, so one that
  writes its `key` and one that leaves it out are the same declaration
  too.
- **Every refusal comes first:** the context, the `schema` grant, the
  author's view, the declaration's own checks, the binding of a model or
  an executor. Only a declaration that would have been applied is
  compared, so the rule never answers where a refusal was.
- **A set is compared one declaration at a time, against the ontology in
  force before it.** Its refusals are judged over the whole set. What
  differs is appended in the set's order, and what that sets off
  happens: a recomputation, a set's memberships. A set wholly in force
  is answered at the head.
- **A proposal is recorded,** in force or not. When it is
  accepted, the proposal is decided and what is in force at that moment
  is not appended again, so an acceptance may append `write_accepted`
  alone.
- **Every kind:** a value type, a type, a function, a verb, a trigger,
  an object set. A function with a backing and a type with a field
  computed outside the lake are no exception (022).
- **A migration to the type in force** is the same case: its `to` is
  the type in force, judged the same way, and it renames nothing.
  `migrate_apply` answers it `applied` at the head and appends nothing,
  after every refusal, a stale report among them: the preflight is made
  and its id compared, and each answers as for any migration. An
  accepted proposal of one appends its acceptance alone. A value type's
  migration to itself needs no rule: its preflight refuses it, there
  being nothing to migrate.

**What a declaration did besides its event follows only one that
differs.** Stated again while in force, a declaration once did this
besides appending its event, and none of it was a promise of this
record:
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

Each of these follows a declaration that differs from the one in
force, or `recompute` (022), as the fold decides it from the `declared`
event. The rule changes nothing of the log format, and a log that holds
identical declarations folds as it did.

### The migration doors refuse in one order

The migration doors refuse in this order, in the frame every write
door shares (016).

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
declared, accepts something other than it does or renames a value, and
keeps its `kind` and `scale` (`invalid` each); its renames (`invalid`,
naming the pair); its own checks; no key field holds a renamed value and
no computed expression holds one (`invalid`, naming the field); the report
id (`invalid`); with renames, every field that uses the value type is shown
to the caller (`not_granted`); every stored value fits (`invalid`, naming
the value type); then, with renames, the set memberships they change are
within the lake's `max_set_changes` (`too_large`, naming the set).

The preflight's own steps answer one kind with the type's name, or
exclude each other (a migration that takes every key field away names
no new one), and are stated, not checked; so are the report id against
the fit of a value type, both `invalid` naming nothing a caller can
tell apart.

### A read shares the shape of its range

A shape changes only where the log says so. Between two events that
declare, migrate or set something, it is one shape. Yet a read built its
own: it decoded every declaration in force, and asked for them a second
time to find the settings. So each read cost as much as the lake
declared, about 20 µs per declared type on SQLite, for a `get` as for a
`describe_type`.

So a handle keeps what a read builds. That is the ontology, the
declarations as `describe` lists them, the settings and the lake's list
of renames, kept for a range of positions over which no event changed
them. Every read at a position in that range shares it. A handle keeps
two: one at the head, and one at the last other position read.

- **What ends a range is one function, `ends_a_range`.** These end one:
  - a `declared` of any kind: a type, its view, a value type, a
    function, a verb, a trigger, an object set, and `recompute`;
  - a `migrated`, which also renames;
  - a write, an ingest, a retirement, or a recorded output of a
    `sys.setting` record.

  A restriction does not end a range: a setting's value holds while it is
  restricted. The function names every kind of event, so a new kind does
  not compile until it says whether it ends a range. One that changes what
  a read is shown must.
- **Nothing of a reader is in it.** What is shared is what the lake
  declares, before any rule of who is shown what: a principal's grants,
  its renames followed and its `only` are applied per read
  (`006 § A call carries one principal with grants, and the store checks every read and write`).
- **A range grows only from where it stands, after the commit.** Once an
  append's fold has committed, a range that ends just before the append
  is carried to its last event, unless an event of the append ends a
  range. A fold that begins anywhere else carries nothing:
  - the first fold after an open;
  - the fold that catches up after a failed fold;
  - a fold after another process wrote.

  There the next read builds its own. A handle takes the lease only at
  its open, so no two handles write a lake in turn.
- **Anywhere else, a read builds for its position.** This holds for a
  read `as_of` an earlier position, a page of a walk at its cursor's
  position, and a read-only handle beside another process's writer.
  Each builds once for its position, and keeps what it built as the
  second.
- **A shape is kept only from a whole projection.** A rebuild clears the
  projection and folds the log again, so a read beside it may read the
  declarations cleared or half folded. That read answers with what it
  read, as any read beside a rebuild does, but its shape is not kept.
  A built shape is kept only where no rebuild began or ended while it was
  built, and the projection is folded through its position. A rebuild
  drops every shape the handle kept, when it begins and when it ends.
  A read-only handle in another process sees no count of the writer's
  rebuilds, only the projection: a build there that began before the
  rebuild cleared it and ended after the refold passed its position is
  kept, until a declaration, a migration or a setting ends its range.

Nothing a read answers changes. The rows in
`crates/lakelands-store/tests/read_shape.rs` count the builds.

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

## What moved

The sections and sentences on what a principal is shown and told moved
to `docs/decisions/006-grants-not-roles.md` at lakelands#817.

| what this record held | where it is now |
|---|---|
| a shape is its author's view: a field hidden from the author is put back as declared, the report is made on the completed shape, and the doors that complete it from their caller's rights (lakelands#475) | `006 § A declaration or a migration is its author's view` |
| no refusal on the way names a field or a relation its author may not know of, and each is said where the same fault of a name the lake does not have is said (lakelands#475, lakelands#662, lakelands#727, lakelands#750) | `006 § A declaration or a migration is its author's view` |
| a reference field to a type its author is not shown is kept, and one the author sends is refused as naming a type that is not declared (lakelands#718, lakelands#716) | `006 § A declaration or a migration is its author's view` |
| a masked or a write-only field stays its author's to remove (lakelands#475) | `006 § A declaration or a migration is its author's view` |
| what a shape names stands as declared only where it stands: a valid time, a key and a `via` over a field its author may not know of (lakelands#726, lakelands#692, lakelands#727, lakelands#718) | `006 § A declaration or a migration is its author's view` |
| a type its author is not shown is a type that is not declared: `add_field`, a migration, its preview, its apply and its proposal; a trigger, a verb and an object set on it (lakelands#538) | `006 § A declaration or a migration is its author's view` |
| to declare the type itself is not granted, nor a set, a trigger, a verb or a function under the name of one its author is not shown (lakelands#538, lakelands#877, lakelands#893) | `006 § A declaration or a migration is its author's view` |
| a tool's name is taken the same way (lakelands#896) | `006 § A declaration or a migration is its author's view` |
| the test comes before the author's view is completed and before anything is appended (lakelands#538) | `006 § A declaration or a migration is its author's view` |
| an accepter is answered as its listing shows it; a bundle takes no place declared since it was proposed, and an entry whose tool's name was taken since by what its reviewer is not shown is not there to it (lakelands#538, lakelands#893, lakelands#896, lakelands#915) | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a migration keeps what a declaration reads: the refusal names the reading declaration only to an author who is shown it, another is told that a declaration reads the field, and the field named is one the author knows of (lakelands#461, lakelands#475, lakelands#495) | `006 § A declaration or a migration is its author's view` |
| the same refusal at an acceptance, to the one who accepts (lakelands#495) | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a view is such a reader, shown only when its definition is, and what an author not shown the view is told (lakelands#504) | `006 § A declaration or a migration is its author's view` |
| a relation's removal refused for a verb on its link type: what a caller not shown the verb is answered, and that it is told on purpose (lakelands#843) | `006 § Some things are still told, on purpose` |
| a relation's removal refused for another relation or a reference field that refers to its link type: what a caller not shown it is answered, and that it is told on purpose (lakelands#920) | `006 § Some things are still told, on purpose` |
| a value type's renames write every field that uses it: a caller not shown one of them is not granted the preflight, the apply or the proposal (lakelands#553) | `006 § A declaration or a migration is its author's view` |
| the counts of a value type's renames stay whole, hidden and restricted records included (lakelands#553) | `006 § Some things are still told, on purpose` |
| what may name an old value is listed to a reader shown it, a declaration as `describe` shows it and an aging rule as a read answers it, and another reader is told that there may be one (lakelands#553) | `006 § A report is answered as its reader may know it` |
| a type is named in that list by what its reader is described of it (lakelands#861) | `006 § A report is answered as its reader may know it` |
| a proposal's report of renames is listed to each reader as it is shown the lake as it stood before the proposal, and the list and the report's id are the reader's (lakelands#553) | `006 § A report is answered as its reader may know it` |
| a proposed migration is listed as its reader may know it: `renames`, `dropped` and `fields`, `rekeyed`, `references`; the totals, `lossless`, the hashes, the id and `more`; a migration of a type its reader may not see (lakelands#499) | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| a report is answered as its reader may know it: one function trims it at the five places that answer one (lakelands#531) | `006 § A report is answered as its reader may know it` |
| `sample_keys`: the keys of records the reader is answered, the first five among them (lakelands#531, lakelands#875) | `006 § A report is answered as its reader may know it` |
| `sample_collisions`: a colliding key is named to a reader who is answered two or more of the records that would take it and reads every field of the new key, in the order of the new key (lakelands#531, lakelands#875) | `006 § A report is answered as its reader may know it` |
| `more`: a report says it where an entry was left out or a count is larger than the keys named beside it (lakelands#531, lakelands#875) | `006 § A report is answered as its reader may know it` |
| a report's id and what it is made over (lakelands#531, lakelands#554) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| a preview read trimmed is applied by the id it carried (lakelands#531) | `006 § A report is answered as its reader may know it` |
| an author is a reader like any other, in its own preview too (lakelands#531) | `006 § A report is answered as its reader may know it` |
| one who may not see the migrated type is answered no report (lakelands#531, lakelands#762) | `006 § A report is answered as its reader may know it` |
| a removed relation is listed under `dropped` by its own name, to one who sees the type (lakelands#531) | `006 § A report is answered as its reader may know it` |
| a renamed reference field's entry is answered to a reader who may know of the field in the shape the migration leaves (lakelands#524) | `006 § A report is answered as its reader may know it` |
| an entry of `restated` is answered as a conversion is, and a member as `describe` shows the field; the preview, the refusal and a proposal's listing pass the one trim, and the feed and an export carry the report whole (lakelands#554) | `006 § A report is answered as its reader may know it` |
| nothing of a reader is in a shape a range shares: grants, renames followed and `only` are applied per read (lakelands#557) | `006 § A call carries one principal with grants, and the store checks every read and write` |

## How it came to be

- The first version: a shape change is declared or migrated, a
  migration converts by a closed table, history included, and keeps the
  type's key: one that named other key fields was refused as not
  supported, and one that would change a key field's value reported the
  record not convertible.
- Added: a value type is loosened by declaration and tightened by a
  preflight (lakelands#109), and an aging rule whose `from` or `to` the
  type no longer admits is skipped by `age`.
- A migration re-keys a type with its references and links, which
  replaced "a migration keeps the type's key"; the moves came as a new
  member of `migrated`, `rekeyed`, and with it a new format version
  (lakelands#114).
- Added: a declaration or a migration may be proposed (lakelands#199).
- Added: an annotation is a change of meaning (lakelands#239).
- Added: a view is never migrated, and the refusal names the way to
  make the change (lakelands#364); a view becomes a stored type by
  being declared, whatever its fields become (lakelands#401).
- Added: a migration keeps what a view reads (lakelands#363,
  lakelands#391).
- Added: the order of refusals (lakelands#338).
- Added: a migration keeps what a declaration in force reads, a
  relation's fields read on its link type among them; the order of
  refusals took "the type it migrates to reads only fields it has" and
  the ceiling of set memberships, after the report (lakelands#461).
- Added: a removed relation carried by link records is listed in
  `dropped` with its live links (lakelands#437).
- Added: a shape is its author's view (lakelands#475).
- Added: a migration's refusal names the reading declaration only to an
  author who is shown it (lakelands#495).
- Added: a value type's tightening may be proposed (lakelands#476).
- Added: the same refusal names a view only to an author who is shown
  it (lakelands#504).
- Added: a proposed migration is listed as its reader may know it,
  where only its `to` was (lakelands#499).
- Added: a report is answered as its reader may know it at all five
  places that answer one, where four gave it whole (lakelands#531).
- A key is made after the keys it reads, where keys were made in the
  order of the types' names. Where that was not the keys' order the
  report's rows come in the new order, and a report made before is
  refused as stale at the apply (lakelands#524).
- Added: a type its author is not shown is a type that is not declared
  (lakelands#538).
- A declaration in force appends nothing, where each statement appended
  a `declared` with the next version, and a version also counts the
  times `recompute` appended a declaration again (lakelands#565).
- A migration to the type in force appends nothing: it used to append a
  `migrated`, raise the version and read every record of the type under
  the writer to change none (lakelands#577).
- The report says every difference between the declaration in force and
  `to`: `restated` was added, a relation that is not carried by link
  records is listed in `dropped`, where it was left out, and an accept
  compares what it would restate. A report's id differs from the id the
  same migration had before, so a preview is taken again
  (lakelands#554).
- Added: a read shares the shape of its range (lakelands#557).
- The author's view keeps a relation its author is not shown
  (lakelands#662).
- Added: a value type's migration renames values (lakelands#553).
- The author's view refuses a reference field its author sends to a
  type it is not shown as one to a type that is not declared
  (lakelands#716).
- Under the author's view a shape that sends a relation of a hidden
  one's name declares it twice (lakelands#727).
- What a shape names stands as declared only where it stands: before, a
  member that named a hidden field was taken because the field existed
  in the lake (lakelands#726).
- A reference field to a type its author is not shown is a field it may
  not know of, where an unchanged one stood as declared
  (lakelands#718).
- A hidden name is judged at the step that judges the type's own names
  (lakelands#750).
- A valid time its author sends is judged by its name first, where one
  equal to the one in force stood as declared (lakelands#692).
- A grouped view is judged in force by its `group_by` (lakelands#788).
- The record names no number of the log format: "(`rekeyed`, log format
  version 2)" and "a line of format version 11" left it
  (lakelands#801).
- Added: a relation's removal is refused while a verb in force is
  declared on its link type (lakelands#843).
- A migration that removes a relation retires its live links, and
  `dropped` counts the links it retires; before, it took them from the
  walk and the link doors alone, and they stayed live to every other
  read (lakelands#938). Added: the removal is refused while another
  relation or a reference field refers to its link type
  (lakelands#920), and in a migration that re-keys (lakelands#938).
- A rename owes a backing the call that carries a renamed value and no
  call that is as it was (lakelands#857).
- An object set its proposer is not shown is not granted to propose
  (lakelands#877).
- A report's `sample_keys` are the first five among the records its
  reader is answered, and a colliding key is named to a reader who is
  answered two or more of the records that would take it and reads
  every field of the new key, in the order of the new key, where it was
  shown to whoever may see its type; `more` follows from the counts and
  the keys named beside them, where it said only that the report holds
  more than its reader is shown (lakelands#875); what may name an old
  value names a type by what its reader is described of it
  (lakelands#861).
- A trigger, a verb and a function under the name of one its author is
  not shown are not granted (lakelands#893), and a tool's name is taken
  the same way (lakelands#896).
- A bundle is refused at its accept where a name it declares was
  declared since it was proposed, as `declared_since_proposed`; before,
  one proposed while the name was free took the place of what was
  declared since, as its proposer's (lakelands#915).
- Moved at the rewrite (lakelands#817): the sections and sentences on
  what a principal is shown and told left for 006, as "What moved" maps
  them, each leaving one sentence and its pointer where a rule here
  leans on it. "Why" and "What it is not", which stood after four
  appended sections, stand once, after the rules.
- Corrected at the rewrite (lakelands#817): "In this release a
  migration keeps the type's key", with its refusal of other key fields
  as not supported and "Re-keying records is a later door", stood
  beside the rule that lifted it. A migration re-keys, and
  `a_rekey_moves_every_record_with_its_values_version_and_provenance`
  holds it.
- Corrected at the rewrite (lakelands#817): "A report's id names the
  whole report". The id is made after the report is trimmed for its
  reader, over what that reader is shown, and
  `a_schema_hash_is_that_of_the_shape_its_reader_is_shown` holds it
  (lakelands#644).
