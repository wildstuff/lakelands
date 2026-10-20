# 062 — A change plans before it acts

## What is decided

### Every change to the ontology has a plan, made on its own path

A change to the ontology is a declaration or a bundle (`apply`), a
field added, a `recompute`, a type's migration or a value type's
migration. Each has a plan: what it would do to the stored records, in
counts and the first keys its reader is answered, before it does it.

`plan` answers a change's plan and appends nothing. `enact` makes a
change against the plan its caller was shown: where the plan made now,
as that caller is shown it, has another id, it is refused `plan_stale`.
The doors of before stay. `declare`, `apply`, `add_field` and
`recompute` are `enact` with no plan named, `migrate` answers the plan's
section of its type, and `migrate_apply` and `migrate_value_type_apply`
are `enact` against a report's id. Every one of them answers in its
outcome the plan that applied (`Outcome::Applied.plan`).

The plan and the act are one path up to the act's append: a
declaration's drafts and its walk of the records it computes again, a
migration's preflight. So a plan refuses what its act refuses, in the
order of 016 and 028, and a walk the act makes is the one the plan
counts, to a caller who may make the change (below). An id is compared
before anything a lossy change is refused for, as a stale report is.

### A change of the types is made by a holder of `restricted`, and planned as its caller is told

`enact`, whatever change it names, `declare`, `apply`, `add_field`,
`recompute`, `migrate_apply` and `migrate_value_type_apply` need
`restricted` beside `schema`, whatever the lake holds, and refuse a
caller without it `grant_needed`, naming `restricted`, before any record
is read. `accept` of a proposed change of the ontology needs it beside
`review` and `schema`, refused alike as soon as the proposal is known to
change the ontology: what it has read by then (that the proposal is
found, listed and undecided) tells nothing of a held record. So the
refusal is the same on every lake. A migration that narrows a relation
is applied by these doors and needs it alike.
The act judges every record, held ones included; were it refused only
where it met a held record, the refusal would tell of that record. So
`enact` and `apply` of one declaration answer alike. A caller without
`restricted` still plans, previews and proposes the change.

To that caller a change's `plan`, and the previews `migrate` and
`migrate_value_type`, only tell, and are judged as it is told, as a
proposal's plan is (below): a held record the change would leave
breaking a constraint that refuses, and a held link a narrowing would
leave past the relation's new cardinality, count no more than a retired
one, which the change does not judge; a declaration's plan decides no
set and judges no setting's held value, as a proposal's does not. So
the plan may answer `lossless` where the act, made by a holder of
`restricted`, refuses. That tells nothing: the act is not this caller's
on any lake, its refusal to it does not depend on the records, and the
holder who makes it is shown the plan the act judges. Twins that differ
only in whether a record is held or retired are planned alike to the
caller, and refused alike.

### What a plan tells

A plan is one shape for every door: a section per type whose records the
change would change, and a value type's migration's report.

- **A type's section is a migration's report** (027), with two more
  members: `recomputed`, the live records the change computes again, and
  `unlive`, per reference field the change may move, the live records it
  would leave naming no live record.
- **"Live" is the lake's own:** a record that exists and is not retired.
  A held record counts as live, to every reader, so a count tells no
  reader of a hold.
- **A constraint that admits** and that the walk would leave broken is
  listed in `breaches`. One that refuses refuses the act (059), and so
  its plan.
- **A re-key's references:** a migration that moves records' keys
  rewrites what names them; each other type it rewrites references of
  has a section of its own, its `unlive`.
- **Past the lake's `max_set_changes`** a change's set memberships
  refuse it `too_large`; a migration's and a value type's plan decide
  them too, at the lake's last time, and refuse as the act does.
- **A type nothing is judged of** (no constraint it may move that its
  author is told of, no reference it computes that its author knows of)
  is neither walked nor counted: its section
  leaves `recomputed` out and says that every record of the type is
  computed again. Counting would cost about what the walk costs on
  PostgreSQL, about a second at 100 000 records, for a number nothing
  depends on.
- **`lossless`** says whether the change can be made as planned: no
  section loses a value or breaks a constraint that refuses. Where an
  answer only tells, a proposal's plan or report, a listing of either,
  and a change's plan or preview to a caller without `restricted`
  (above), it is judged on what its reader is told of: restricted
  records and constraints over fields it does not read are left out.
  Its accept still refuses on every record.

A reference the change leaves naming no live record is told and never
refused: the act stays its author's (032).

### Each finding as a person can be told it

`lakelands-server`'s `say` writes a plan as a sentence per finding,
each standing alone, its count first and then its first keys: the
words a host shows an operator who is not technical. The CLI prints
them, and MCP and REST send them as `said` beside the plan. They are
not part of the plan or its id.

### A proposal keeps its plan, and is accepted against the plan at its head

A proposed change answers its plan to its proposer. A proposal is
refused only by its declaration's own checks, as before: its plan only
tells, and what its walk finds, a constraint left broken or a set past
the ceiling, is the accept's to refuse. A constraint its proposer may
not read is neither judged nor named in it, so it is answered as if the
constraint were not there, and the plan is lossy only by a record its
proposer is told of. So is a proposed migration, refused as lossy only
by what its proposer is told of, of its own type and of each type whose
references a re-key rewrites, and a value type's migration, refused by
a constraint left broken only on that. A record that would not convert
or would collide, or a value that would not fit, makes it lossy to
every reader alike: retired records are converted, keyed and counted
with the rest, so a restricted one tells nothing a retired one would
not. A proposal of declarations keeps its plan in the log
(`write_proposed`, log format 17), and a proposed migration its
report, its plan's section, as before; both are made as the proposer is
shown them without `restricted`, so no count kept tells of a hold. Each
reader is shown them as its listing shows a report, a first key only
where it is answered now. A stored plan of declarations is its
proposer's, its counts made over the lake as it stood then. To a reader
kept from a field a count may rest on a field it may not read, there or
in a lake declared since, and to a holder of `only` it counts records
outside its pairs; neither is shown a stored plan of declarations, and
one who holds `schema` asks `plan` for one made now, the one
`accept_planned` compares. A reader of every field is shown it with its
first keys answered now. Its plan is lossy by the breaches its reader
is told of, each judged by the bundle's own declaration of its
constraint, so one the bundle loosens to admit leaves its section
lossless. A plan trimmed for its reader leaves the entry's `more` as it
was: what the trim leaves out is what its reader may not be told, and
counting it would tell twins apart; the act judges every record all the
same. A proposal's plan judges no setting's held value against the kind
declared: the accept does, and the plan would tell of a value its
proposer may not be answered. A migration's report kept before reports
counted what they compute again is read as computing none.

A plain `accept` of a proposed change acts as before, and answers the
plan that applied: a re-key's with the sections of the other types it
rewrites, as `plan` answers them, and so does the proposal's answer.
`accept_planned` names a plan, which it requires: it accepts only where
the plan of the proposal's change made now, as its reviewer is shown it
by `plan`, has that id, and is refused `plan_stale` otherwise, the
proposal left pending. So what the reviewer read is what applies.
Refusing every accept whose plan moved since the proposal was not taken:
on a lake that is written, counts move between the two, and such an
accept would never pass.

### A plan tells each reader only what it may see

A plan is shaped for its reader as a report is (006, 024): a section of
a type it may not see is left out, and so is one of a type none of whose
fields the change computes again it knows of, those derived from them
included, since its count would tell which field it does not know of the
change computes. A section counts what it computes again only where
something it is told is judged, a constraint or a reference it knows of.
A proposal of declarations computes no held record: it counts none, and
the record's inputs are not its proposer's. A held record is counted and
named only for a holder of `restricted`, in `recomputed`, `unlive`,
`filled` and `breaches` alike; a constraint that refuses still refuses
on every record, save where the plan only tells (above). `unlive` is
told to a reader who reads the reference and is shown its target type;
a count over a hidden type would tell which of its keys exist. A plan has no `more` of its own: a reference
whose target type its reader is not shown is a field it does not know of
(024), so its reader reads no such reference. A section's `more` keeps a
report's meaning, counts past their first keys. A plan's id is made over
what its reader is shown and the head, as a report's id is. A write to
the lake moves the head, so a plan of an earlier head is stale even
where its counts did not move. No plan id is logged by `declare`,
`apply`, `add_field`, `recompute` or `enact`; a `migrated` keeps its
report's id, as before; and a proposal keeps its plan as it is stored,
with that plan's own id.

## Why

The first real installation has an agent propose changes to the
ontology and a person accept them. Before this, the person could not see
a change's size before it landed: a migration's preview said nothing of
the references it would leave naming no live record, a declaration had
no preview at all, and a proposal carried its change but not its
consequences. A plan made on another path than the act's would drift
from it; made on the act's own path, the two cannot disagree.

Measured on SQLite and PostgreSQL at 10 000 and 100 000 records, a plan
costs its walk: 3–6 ms per 1 000 records on SQLite and 12–18 on
PostgreSQL for a declaration, 4–16 for a migration's preview. Its
look-ups of the records references name read a page of 1 000 keys at a
time (`get_many`), 1.5–4 ms per 1 000 keys, where a read a key costs
forty times that on PostgreSQL (lakelands#1141).

## What it is not

- **Not a refusal of the act for its consequences.** A plan informs; the
  person decides (032).
- **Not a second implementation.** A plan is the act's path, stopped
  before its append.
- **Not a new engine method.** The look-ups use `get_many`, which every
  engine has.
- **Not words in the plan.** Sentences are the server's, outside the
  plan and its id, as a failure's are.
- **Not `declared` in the plan.** The positions an act would declare at
  differ between a plan and an accept, which appends its acceptance
  first. The outcome's `declared` tells them as they landed.
