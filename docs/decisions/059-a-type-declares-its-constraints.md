# 059 — A type declares its constraints

## What is decided

Decided on lakelands#1119; built in 0.15.0. A type names what every
record of it holds, and the store holds it at every door a record is
written through. Before this, a host held such a rule in its own code,
at the doors it remembered. A rule held there is skipped by an import,
a source or an agent that writes directly, and the lake cannot say that
its records hold it.

### A constraint is a condition over the record, named and declared on its type

A type declares `constraints`. Each one has:
- a `name`, an identifier unique within the type;
- `holds`, a condition in the store's expression language;
- a `description`, if any;
- `at_ingest`: `refuse` (the default) or `admit`.

A record holds a constraint when its condition is true on the record
as written. A comparison with a value the record does not hold is
false, as in every condition of the lake. A constraint over an
optional field says what it holds when the field is absent:
`paid is null or paid <= amount`.

The constraints are not in the schema hash. Like `insert_only`, they
judge writes and leave a record's shape alone.

### A constraint reads its record alone

A constraint reads:
- the record's own fields, as stored or as computed at the write;
- `previous(f)` and `changed(f)`, so a transition can be ruled.

The declaration refuses `constraint_reads_beyond`, naming the
constraint and what it reads, when the condition reads something that
can change without a write of the record:
- a path through a reference;
- an object set (`in_set`) or `exists`;
- a setting;
- the clock;
- a field computed at the read;
- a field computed outside the store: an embedding, or a function a
  backing serves.

Anything else would make a record break a constraint with no write to
refuse, and an answer that changes with the clock cannot be held.

### Every write door judges a write that moves what a constraint reads

`upsert`, a verb's write, a reaction's write and `ingest` judge each
constraint on the record as the write leaves it, with `previous` as it
was before. A write is judged by a constraint when it creates the
record, or when it moves a field the constraint reads: directly,
through a derived field or a field an `expr` function computes at the
write, or through a field the fold clears when its inputs change. What
a write may move is read from the declaration, never from the values,
so whether a write is judged tells nothing of the record. A relation's
link fields declare no constraints, since a link type is made from its
relation, so a link's write is judged by none. A write that moves
nothing a constraint reads is not judged by it.

A write that would break a constraint is refused
`constraint_broken`. The refusal names the constraint, the type and the
key, never a value. The write appends nothing.

### Ingest refuses a breaking record, or admits it

A constraint that refuses at ingest refuses a source's record as it
refuses an author's write. One that admits lets the source's record in:
a source states what is, and a lake that refused it would hold less
than the source says. A record admitted while it breaks a constraint
takes every write that leaves what the constraint reads as it was. Of
what it reads, it takes only a write that makes it hold.

### One who does not read every field a constraint reads is not granted a write it judges

A writer that does not read every field a constraint reads is refused
`not_granted` for a write that constraint judges, whether the write
would hold or not. A refusal that depended on the value would tell the
writer something about a field it may not read. Its writes that move
nothing the constraint reads are judged as before.

A constraint is told by `describe_type` to one who reads every field it
reads. To any other author, kept from such a field or shown it masked
or write-only, it does not exist: a declaration or a migration that
leaves it out leaves it as declared, as a field the author may not know
of is left (024 and 027, extended), and a migration's preview names it
nowhere, neither in `breaches` nor in `restated`.

At accept, a proposal whose write now breaks a constraint is refused
`constraint_broken` to a reviewer who reads every field the constraint
reads. Any other reviewer is told `write_changed_since_proposed`, as
for every refusal it may not be told (013).

### A constraint is added or changed by a migration

`declare` refuses a constraint added to a type already declared, or
tightened there, with `constraint_needs_migrate`, naming the type and
the constraint. It takes a loosening, which breaks no record that held
before:
- a removal;
- the same condition in other words, compared by its parsed form;
- another `description`;
- a move from `refuse` to `admit` at ingest.

A migration whose `to` declares constraints lists, in `breaches`, each
constraint that live records would break once converted and filled. An
entry holds the number of records and the first keys its reader is
answered. A constraint that refuses at ingest makes the migration lossy
when it has an entry, so its apply is refused. One that admits is
listed and applies. A reader who does not read every field a
constraint reads is not told its entry, and is told `more`.

A migration's preflight judges records as converted and filled. A
field it computes anew is computed by the recompute that follows the
migration; a breach that recompute would make is not listed yet.
