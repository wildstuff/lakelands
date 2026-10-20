# 035 — A link is a record

## What is decided

### A relation declared with neither `via` nor `derived` is carried by link records

A relation declared with neither `via` nor `derived` is carried by
**link records**: one record per linked pair, of a type the store
derives from the relation, `sys.link.<type>.<relation>`, keyed by the
pair (`from`, `to`) and holding the relation's own fields.
`docs/log-format.md` §2.3 specifies it. Links are written and removed
by the record events the log already has (`record_upserted`,
`record_ingested`, `record_retired`); no event kind is added.

### A relation carried by links has a pair of doors, `link` and `unlink`

The catalog gains a pair of doors per relation carried by links:

- `link(who, type, key, relation, to, fields, opts)` writes the link from
  the record `key` of `type` to the target `to`, with the link's fields
  as a sparse patch; writing it again changes its fields.
- `unlink(who, type, key, relation, to, opts)` removes it.

Both answer an `Outcome` and check the principal's grants on the link
type's fields, as `upsert` does; `link` also checks the relation's
`authority` (who writes its links: `authored` by default, `source` for
links that arrive by ingest). The tool generator makes one tool per
door and relation, named after the type and the relation, so that two
types' relations of one name cannot collide. The WIT package and its
drift check follow the new doors.

- **A relation's authority** may change by an ordinary declaration: it
  swaps the door the links arrive by and keeps the links already
  written. A `source` relation's own fields are `source` (or derived):
  an `authored` field on a link no door writes is refused at
  `declare`.
- **`unlink`** removes a link whatever the relation's authority, as
  `retire` removes a record whatever its fields' authority. It is
  governed by the principal's grants on the link type. Ingest writes a
  source relation's links, but cannot retire one. A principal who is not
  shown the type that declares the relation is shown nothing of its link
  type, at these doors and at every other
  (`docs/decisions/006-grants-not-roles.md`, extended).
- **Tools and rights.** `describe` names, per type, the relations whose
  links the principal may write: those whose links are `authored` and
  whose link type's `from` and `to` the principal may write. The tool
  generator makes `<type>_<relation>_link` and `_unlink` for them, and
  checks their names for collisions with every other tool. A link type's
  own description offers `find` and `get`, never `upsert` or `retire`.
- **The WIT package** gains the two doors, `relation-decl.authority` and
  `type-rights.link`.

### The link doors refuse in a stated order

The link doors refuse in this order, in the frame every write door
shares (016, extended): the lake, the context, the door's own checks,
the append.

**`link`**: the type is declared and shown to the caller
(`not_found`), and so is the relation (`invalid`). The type declared and
shown, and the relation shown where it is declared, alone give way to a
retry: a call sent again under its key is answered as the first was,
though the type is no longer there for its caller or the relation no
longer shown to it. A relation a migration removed since does not give
way: the call sent again is answered that the type has no such relation;
it is not a view (`invalid`, 018, extended); it declares the relation,
carried by link records (`invalid`); the relation's links are authored
(`wrong_door`); `from` and `to` are not among the fields (`invalid`,
naming the field); the retry, a retry that itself fails included; the
record checks of 016 on the link record; the relation's
cardinality over the live links (`constraint`, naming `to`), after the
record's grants so that a principal who may not write the link learns
nothing of the links there are (024, extended).

**`unlink`**: the type and the relation are declared and shown, a
retry excepted as for `link`, it is not a view, it declares the relation
carried by link records, as for `link`; then the retry and
`retire`'s checks on the link record: its grants (`not_granted`), the
link is live (`not_found`), the expected version (`conflict`). An
unlink is not refused for the relation's authority (above).

A view declares no relation carried by link records, and an undeclared
type is no view, so the steps from the type to the relation answer
`not_found`, then `invalid` naming the type, and are stated, not
checked, between themselves.

### The pattern query walks a link relation as it walks a `via` one

The pattern query walks a link relation as it walks a `via` one; an
engine answers its `edges` from the live link records at the view's
position, with the link's fields in `Edge.fields`.

- **Reading links.** Every engine answers a link relation's edges by one
  rule, `lakelands_core::links::link_edges`, over the live link records
  at the view's position. At the head, SQLite and PostgreSQL find the
  candidates through the exact-text index they already keep on `from`
  and `to`. At an earlier position they read the link type's records
  there. The engine seam and the projections' schemas are unchanged.
- **Retiring an end** keeps its links, as retiring a referenced record
  keeps the references (032). The walk does not reach the retired record.
- **A restricted link** is walked only by a principal holding the
  `restricted` grant. An edge's fields are those of the link type the
  principal may read: a link's edge comes from its own record, not from
  the records it joins.

### A change to a relation's fields, target or cardinality is declared or migrated, on the link type as on any type

A relation's fields are shape
(`docs/decisions/007-schema-hash-covers-shape.md`), so a change to them
is declared or migrated by
`docs/decisions/027-shape-changes-are-declared-or-migrated.md`, on the
link type as on any type.

- An added optional link field is a declared change: the owner type
  is declared again, and its link type with it, in the same append.
- A change that reshapes link records (a retyped field, a relation
  retargeted, or removed and declared again with another target) is
  refused at `declare` and goes through `migrate` on the owner type.
  Its preflight reports the link records beside the type's, each field
  named with its relation, and `migrate_apply` migrates each changed
  link type in the same append. A retargeted relation's live links convert only where
  their `to` names a live record of the new target, as any reference
  does (`docs/decisions/032-a-reference-names-a-live-record.md`,
  lakelands#1071).
- A link field is never renamed: a rename is a drop plus an add.
- A migration's `to` meets every check of its link types that
  `declare` makes, a reshaping one too: a link field only those checks
  refuse is refused by the preflight and by the apply, naming the link
  type, and an embedding no bound backend makes is refused as unbound
  there, as on the type itself (lakelands#1124). One it renames, or a
  relation it renames, is compared with the one it is renamed from:
  its vectors are carried, so it needs no backend. A name a rename frees
  and `to` declares again is a field anew. What it needs bound is asked
  after its renames are judged, against the links stored too, so a
  malformed rename is told so whatever is bound. A link field that a
  link field its author may not know of reads stands to that author as
  it is (027): no check of the link type names the hidden one.
- The link type is classified at every declaration, against the one in
  force. A relation dropped by `migrate` and declared again with
  another target, or with a field retyped, would reshape the retired
  links its link type holds as history, so it is refused, naming the
  link type, and goes through `migrate`.
- Narrowing a relation's cardinality is a migration. Its preflight
  counts the live links that would break the narrower rule, and
  refuses while any would.
- Removing a relation is a migration. Its preflight lists the relation
  under `dropped` with the number of its live links, as it lists a
  removed field with its values
  (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
  extended), and its apply retires each of them in its own append, as
  a purge retires what it takes: one `record_retired` a link, caused by
  the `migrated`, with what a retirement sets off. The links stay as
  history, and a read as of before answers them.
- A relation's removal is refused while another relation targets its
  link type, whichever type declares it, or a reference field refers to
  it: either would name the links of no relation (lakelands#920). Its
  author retargets or removes it first, or removes both in one
  migration. A removal in a migration that re-keys the type is refused
  too, naming the relation: the re-key would move the links the removal
  retires, so the two are two migrations, as a rename and a re-key are.
- A relation may target a link type, so a link's end may be a link; but
  no declaration closes a ring of link types, each the end of the one
  before, by a type, a set or a migration (`link_type_ring`, naming the
  ring). A link of a ring needs a live link of the next one as its end,
  so none can be written, and the links written before the ring closed
  would be judged by ends no write could make again (lakelands#919).
- The link type is checked as any type is, when its owner is declared:
  its fields' kinds, functions and embeddings. A field that cannot work
  fails at `declare`, not at the first write.
- A relation removed from its type keeps its link type and its retired
  records as history, and its link doors refuse it. Declared again, in
  any shape, it starts with no link: a removal made by mistake is
  mended from a read as of before, not by declaring the relation
  again.

### A trigger on a relation watches its link type

A trigger on a relation watches its link type: `on` names
`sys.link.<type>.<relation>`. It fires on every write of a link, as a
trigger on a type fires on every write of a record: `link`, a link
written again, `unlink`, and each link a source ingests, link by link.
Its guard reads the link's fields (`from`, `to` and the relation's
own), and `changed(f)` holds on a write that changes `f`. On `unlink`
the guard reads the link's fields as they were, and none reads as
changed. A host tells a link from an unlink by the write the
`trigger_fired` event's `cause` names: `record_upserted` or
`record_ingested` against `record_retired`. A relation carried by a
reference field has no link type, since its links change with the
field: a trigger naming one is refused on `on`, pointing at
`changed(<field>)` on the type. The trigger declaration gains no
member, so the log format does not change.

### A verb on a link type writes a link's own fields and never an end

A verb declared on the link type (003, extended) writes a live link's
own fields and nothing else: under the relation's authority, with both
ends checked as `link` checks them, live and answered to its caller,
and never an end. A `set` that names `from` or `to` is refused when the
verb is declared, as `link` refuses an end among its fields. Each door
keeps the order of its family: a verb's call on a link answers a stale
expected version before it judges the ends, as any verb compares the
version where it reads its record (024), and `link` judges the ends
first, as any record write judges its sent references before the
version (016).

### A renamed relation keeps its edges

A relation's name is part of a type's shape, and names change. In a
migration that names no rename, a relation that disappears under one
name and appears under another is a removed relation and a new one: its
links stay behind under the old link type. A migration names the
rename, `relation_renames`, old name to new:

- **The edges are the same edges.** A relation carried by a `via` field
  or derived from matching values has no record of its own, and is named
  anew. One carried by link records moves them: at the `migrated` event
  the fold takes every record of the old link type to the link type of
  the new name, under the same key, with its version, fields, provenance
  and flags, retired and restricted ones too. No event is written for a
  link. The old link type's keys end there, so a read as of before
  answers the links under the old name, and a set's memberships move
  with the records.
- **What names the old link type is carried.** A trigger or a verb on
  it, an object set of it and a view that reads it are declared again by
  the lake under the new link type, in the migration's append, after the
  new link type's declaration, and the preflight's report lists them. A
  relation's `inverse` is its own member and is stated in the new shape.
- **A rename moves and changes nothing.** The carrier, the target, the
  cardinality, the link fields and who writes them stay; a migration
  that also re-keys records is refused, as is one that changes the link
  fields, each naming the relation. A rename onto a name whose link type
  holds the retired links of an earlier, removed relation is refused, and
  so is one whose link type a reference field or a relation of another
  type names: that type is named to an author who is shown it, and
  another is told that a declaration refers to it.
- **A link type no relation carries takes no link.** The old link type
  stays declared, for the reads as of before, and so does a removed
  relation's. An ingest into one is refused, naming it, and a verb
  declared on one is refused as on a type that is not declared
  (lakelands#759); a verb in force moves to the new link type with a
  rename. No verb in force names a link type no relation carries: a
  verb is the one door left there, so a relation's removal is refused
  while a verb is declared on its link type, and its author declares the
  verb again on another type first (027, extended; lakelands#843). A
  trigger, a view and an object set of it stay, and may be declared
  there anew. The removal's retirements set them off as any retirement
  does: the trigger fires for each link, and the set and the view
  follow. After it the link type holds no live link: neither `link`,
  `unlink` nor an ingest reaches it, and a purge finds nothing there to
  take. Where a renamed
  relation takes a name whose link type stands, empty, in another shape,
  that link type is declared again in the shape of the links it takes;
  a declaration that reads a field of it which the renamed relation
  lacks refuses the rename, as it refuses a migration that takes a field
  away.
- **Grants follow the link type** as they follow a field
  (`docs/decisions/006-grants-not-roles.md`, extended): a restriction in
  both directions of time, a permission never.
- **A read as of before is told the names since.** A host knows a
  relation by its name at the head, and as of an earlier position the
  link type stands under the old one, so it could not tell a relation
  renamed since from one declared since. `describe` and `describe_type`
  as of a position tell each type they describe the entries of the
  lake's list of renames that named it again after that position,
  oldest first, under `lineage.renamed`: the same list the grants
  follow, never a copy. The last entry's `to` is its name at the head.
  A type not described there was not declared there, or is not the
  reader's to see. At the head nothing is told: there is nothing after
  it. Who is told is `006`'s.
- **The log's format** takes `relation_renames` as a member of a
  `migrated` event; a migration that renames no relation leaves it out.

## Why

A link with fields is a fact with a history: when it was made, by whom,
from which source, what its fields said then. Records already carry all
of that: versions, per-field provenance, `as_of`, retirement that keeps
history, rebuild, verification, the change feed. Two new event kinds
would have to grow every one of those again, and every reader of the
log would need a step of the format to read them. Carrying links as
records gives them everything records have, and changes nothing a
reader of the log knows.

Keying a link by its pair makes a relation a set: a second write of the
same link changes its fields, never adds a duplicate, and cardinality is
a check over live links rather than a convention.

Naming the link type under `sys.` keeps the writes where the rules are:
the `sys.` guard stops a caller from declaring or reshaping it, and the
ordinary doors refuse its records, so a link is made only through the
doors that check the relation's authority and cardinality.

## What it is not

- It is not a second way to link. A relation is carried by its `via`
  field or by link records, never both, or has no carrier at all: a
  derived relation connects records by matching values when the store
  reads, and stores no link
  (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
  A relation carried by `via`, by link records or by matching values
  keeps that carrier, and a declaration that would move a relation from
  one to another is refused.
- It is not a graph store beside the lake. Link records live in the same
  log and projection as every record, and the store's walks read them
  through the same engine seam.
- It does not widen what Lakelands reserves by roster
  (`docs/decisions/003-reservation-by-namespace.md`): link types are
  recognised by their name, `sys.link.` and the relation, as every
  system type is.

## How it came to be

- The first version: a relation without `via` is carried by link
  records, written through `link` and `unlink` and walked as a `via`
  relation is; both doors were said to check the relation's authority,
  and the log format was said not to change.
- With the code: how links are read, how a relation's fields, its
  cardinality and its authority change, the tools and the rights, and
  `unlink` removes a link whatever the relation's authority.
- A trigger on a relation watches its link type (lakelands#194).
- A relation may be derived instead, with no carrier, and the carrier
  rule covers all three kinds (lakelands#252).
- The order in which the link doors refuse is stated (lakelands#338).
- A migration's preflight lists a removed relation with its live links
  (lakelands#437).
- A migration names a relation's rename and its links move with it;
  before, a renamed relation was a removed one and a new one. The log's
  format took a member for it, `relation_renames`, so the first
  version's sentence that the log format does not change held no longer
  (lakelands#429).
- A principal not shown the type that declares a relation is shown
  nothing of its link type (lakelands#595).
- A verb may be declared on a relation's link type, and none on a link
  type no relation carries (lakelands#759).
- The first rule's sentence on the log format and the rename's item on
  the log's format name no format version by its number
  (lakelands#801).
- The order of the link doors says what gives way to a retry, and that
  a retry which itself fails has the retry's place; the doors answered
  so before, and the order had not said it (lakelands#799).
- A relation's removal is refused while a verb is declared on its
  link type; before, the verb stayed in force and went on writing links
  no relation carried. The record says that a trigger, a view and an
  object set stay there over the links it keeps, as they did before
  (lakelands#843).
- The order is narrowed on what gives way to a retry: the relation
  shown where it is declared, and not one a migration removed since.
  The order had said the relation declared and shown, and the doors
  answered so before (lakelands#843).
- The record says the order each door keeps: a verb's call on a link
  answers a stale expected version before the ends, `link` the ends
  first; the doors answered so before (lakelands#844).
- Corrected at the rewrite (lakelands#817): the WIT package was said to
  stay at `lakelands:data@0.2.0`, not yet released; the package takes
  the number of the release that first ships it (034), and `wit/` says
  so.
- A migration that removes a relation retires its live links; before,
  they stayed live under a link type no relation carries, at every read
  but a walk, authored ones had no door left, and the relation declared
  again in the same shape took them back. A relation's removal is
  refused while another relation or a reference field refers to its
  link type, and in a migration that re-keys (lakelands#938,
  lakelands#920).
- A ring of link types is refused at its declaration; before, a
  migration or a set could close one, and its links were read as far as
  those written before it closed (lakelands#919).
- At the rewrite (lakelands#817): the sentences in "Why" on what two
  new event kinds would cost every reader of the log name no format
  version by its number; `docs/log-format.md` has it. The first rule's
  sentence that the log format does not change, untrue since
  lakelands#429, left.
- A trigger on a relation fires on each link a source ingests, link by link: "per window" said of it what no answer turns on (lakelands#886).
- Extended: `describe` as of a position tells a link type the names its relation's renames gave it since (lakelands#1291).
