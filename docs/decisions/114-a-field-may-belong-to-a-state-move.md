# 114 — A field may belong to a state move, so taking a source back does not count it as an author's value

A source brings an invoice: its `status` is `open`, a field that declares
an `initial` value. A verb marks the invoice paid, and its move writes two
fields at once: `status` becomes `paid`, and `paid_date` takes the day,
where an operator would otherwise have written it by hand. The record is
still the source's. When the source is taken back — `purge_source`,
`forget_source`, `remove_values` — the record is the source's and goes with
it.

It does not. A purge takes the records a source brought and refuses one
that co-owns: a record holding a value an author wrote
(`docs/decisions/039-a-source-is-retracted-as-one.md`). A verb's write is an
author's write, and the provenance of both fields is `authored`, so
`paid_date` co-owns and the purge refuses the record — though no person
chose the value; the move did. `status` escapes, but for another reason: a
field declaring an `initial` value co-owns nothing, whoever wrote it last
(`crates/lakelands-store/src/purge.rs:158–169`). So a move that marks a
record `paid` takes the source's record out of reach of the take-back, and
`status` is spared only because it declares something else.

The ontology must be able to say that a field's value belongs to a move.
This record adds that to the field's declaration.

## What is decided

### The declaration is on the field, and it is a boolean

A field's declaration takes `state_move: bool`, `false` when absent, and
written only when true
(`#[serde(default, skip_serializing_if = "std::ops::Not::not")]`, as
`erasable` is written beside it in
`crates/lakelands-api/src/ontology.rs`). It is a member of a field, not of
the type: `TypeDecl` carries no list of verbs, and the question — is this
value the move's? — is asked of one field at a time.

It is not `authority: verb` (lakelands#1130). That authority writes a field
through the type's verbs alone and refuses `upsert` and `ingest`, so a
field carrying it cannot also be written by hand — and the field this
record exists for is written by both: the operator who fills `paid_date` in
by hand must stay able, and the verb that fills it as part of the move must
be recognisable as a move.

It is not inferred from the verb. A verb's `set` names the fields it writes,
and a field a verb sets is not by that alone part of a state move: a verb
that appends a note or bumps a counter has written a value an author could
have written, and its value should co-own as that one does. The declaration
is where the operator says which of a verb's writes are the move.

### What it changes about a record

A value of a field declaring `state_move` is no owner's when the write that
last set it was a move. The take-back does not count it: the record it is in
is not kept for it, it is not reported as an author's value taken, and a
record the source brought goes with the source.

A value of the same field an author wrote directly — through `upsert` or in
an accepted proposal — co-owns as any authored value does, and so does one a
source wrote. The last write decides: an operator who writes `paid_date`
after the verb ran brings the record back under the older rule, and a later
move that writes it again puts it back under this one.

### The marker is the provenance, and the last write decides

The marker is `FieldMeta.origin == Origin::Authored` with
`FieldMeta.procedure.is_some()`
(`crates/lakelands-api/src/record.rs:37–62`). The fold materialises
`procedure` from the event that last wrote the field
(`crates/lakelands-store/src/record.rs:294–295`), so the question is asked
of the writing event, not of the record's history.

A verb's write names its procedure (`crates/lakelands-store/src/writes/record.rs:1128`).
Nothing else an author writes does: `upsert` names none, and an accepted
proposal applies authored writes with no procedure
(`crates/lakelands-store/src/writes/propose.rs:648, 671`). The producers
that do name one are not `authored`, and are not mistaken for a move: a
function's or an embedding's value is `Derived` with its name, an aging
rule's is `System` with `aging:<rule>`
(`crates/lakelands-store/src/aging.rs:252`), and a proposal's rejection is
a system event. A field those write cannot declare `state_move` in any
case, below.

The marker is coarser than the move, and the declaration carries the
rest. A write through any of a type's verbs carries that verb's
`Procedure` — its name and version
(`crates/lakelands-store/src/writes/record.rs:1128`) — so once a field
declares `state_move`, a write of it by a verb that sets it outside the
state change is the move's too. Nothing in the record or the declaration
separates the two: the record carries the verb's name and the
declaration names no verb; a verb's `set` names a field, not the part of
its write that is the move. The declaration is therefore the operator's
assertion that no other verb writes the field, and `authority: verb`
makes that structural — no door but the type's verbs writes such a
field. The sixth row below pins the coarse answer, so the limit is part
of the contract rather than left to be found: a verb's write that is not
the move co-owns nothing too.

### One question, asked in one place

The `initial` skip stands twice today, once in `co_owned`
(`crates/lakelands-store/src/purge.rs:165–169`) and once in `holder_of`'s
`holds` (`crates/lakelands-store/src/remove.rs:603–607`), and `forget`
imports `co_owned` (`crates/lakelands-store/src/forget.rs:74`). The three
acts and their preflights already answer through it —
`purge_source` and its preflight (`purge.rs:635`), `forget_source` and its
preflight (`forget.rs:825`), `remove_values` and its preflight
(`remove.rs:919`) — so this record folds the two copies into one predicate
in `purge.rs`, and every act and every preflight keeps answering through
it:

- `no_owner(decl, f, meta)` — the take-back's question: the field declares
  an `initial` value, or it declares `state_move` and the value's last write
  was `authored` and carried a procedure. `co_owned` and `holds` both filter
  on it, so a purge, a forget and a removal retire the same records and keep
  the same ones.
- the provenance half alone is what the `moved` report excludes
  (`purge.rs:520–525`): an `initial` field an author wrote is exactly what
  `PurgeMoved` reports, so that report cannot use the whole predicate; a
  move's value is no author's and is not reported. Keeping that half is
  also the change's regression test, and the filter is judged by it:
  `INITIAL_NO_OWNER` (registered at
  `crates/lakelands-core/src/conformance/checks/purge.rs:657`) asserts
  `moved(2, ["o1", "o3"])` over two author-written `initial` values
  (line 696), and a `moved` that read the whole predicate would name
  neither.

`PurgeMoved`'s own sentence — "the records a purge takes whose one field
with a declared `initial` value an author had written"
(`crates/lakelands-api/src/write.rs:1936–1937`) — gains the clause that a
value a move wrote is no author's and is not named.

### The declaration standing at the act

The declaration is read in force when the act runs, as `initial` is:
`settle_at` (`purge.rs:367`) and `holder_of` (`remove.rs:585`) take the
current `TypeDecl`. So declaring `state_move` on a field of a type that
already holds records changes what a later take-back does to them, and
rewrites no record; a record is re-read at its next take-back, not before.

It is droppable, unlike `erasable` (`ErasableDropped`,
`crates/lakelands-api/src/error/case.rs:984`). Dropping it returns the field
to the stricter rule — its values co-own again — and unsays no act already
taken: the declaration writes no value and seals nothing; it says only how
a take-back reads a provenance. A field that once declared it may be
declared without it.

### Where it is refused, and where it is not

Four refusals, in the shape of `initial`'s validator
(`crates/lakelands-store/src/declare.rs:527`) and beside it:

- `StateMoveNotOfType` — "only a field of a type declares it": a verb's
  payload, a function's input and a relation's own field are not a type's
  field, and `field_type` is given no type for them.
- `StateMoveInView` — "a view's records are computed, and none is taken".
- `StateMoveKey` — "a key's field is given by the record's key".
- `StateMoveNotAuthored` — "only a field whose authority is `authored` or
  `verb`, and that no step derives, declares one". The predicate is
  `initial`'s exactly (`declare.rs:527`):
  `matches!(f.authority, Authority::Authored | Authority::Verb) &&
  f.derived.is_none()`. It says what the authority is, not who writes: an
  `authored` field a source is admitted to write still passes
  (`Door::admits`, `crates/lakelands-core/src/rights.rs:1358`), so the
  sentence does not claim no source writes it.

`Place::StateMove` prints "`state_move`" beside `Place::Erasable`
(`case.rs:641`), and `case/samples.json` gains a sample of the place and of
each case, as it holds `erasable`'s (`samples.json:212–224, 2822`).

It is allowed:

- beside `initial`, which the issue's `status` does: the two say different
  things — that the value is there because the record is, and that the last
  write was a move;
- beside `authority: verb`: coherent, and no coarser there than the
  operator already made it — no door but the type's verbs writes such a
  field, and declaring it the move's says every such write is the move's;
- on a field that names a record, and on the field a relation's `via`
  names. `initial` is refused there because its value is written when the
  record is made, where it cannot be checked against a record that does not
  exist yet; `state_move` writes nothing, so it has no such reason.

### The hash does not move, and nothing is migrated

`shape.rs` hashes exactly a field's `name`, `type` and `required`, and its
`authority` where that is `writer`
(`crates/lakelands-api/src/shape.rs:164–179`). `state_move` is outside the
hash for the reason `erasable` is: it says how a take-back reads a value,
not what a record holds (`docs/log-format.md:501–505`). So no `migrate`
follows and no stored record is reshaped; a declaration that adds or drops
it is a `declared`, whose `version` steps, and "a new version here never
reshapes stored records" (`docs/log-format.md:470`).

### The contract

The change is additive, and not breaking:

- the declaration member, absent from every document that does not set it,
  and `false` everywhere it is absent, so a lake that never declares it
  behaves exactly as today and a take-back's answer changes only on a field
  that declares it;
- `describe` answers it: `FieldDescription` gains `state_move` beside
  `erasable` (`crates/lakelands-api/src/describe.rs:182, 234`);
- a probe without the right to read the field is still shown it, as it is
  shown `erasable`: it is not in `unread!` (`ontology.rs:551`) and so
  survives `shape_alone`; it says nothing about a value the prober may not
  read;
- the private `InputWire`, which refuses a member it does not know and
  whose two conversions name every member of a field
  (`crates/lakelands-api/src/ontology.rs:390–433, 434, 479`), is given the
  member, so a function's input or a verb's payload naming it is refused
  by the declaration's own sentence rather than as an unknown member.

Owed with the implementation, in the same pull request as the code: the
field-member table in `docs/log-format.md:1225` (which omits `erasable`
today as well) and the not-hashed paragraph at `docs/log-format.md:501`.
Decisions 039 and 065 are not owed with the code: their sentence is part
of this change, below.

### The two records it amends

This change amends decisions 039 and 065, as the issue's triage orders:
the design settles the question before any code, so the rule is restated
where the take-backs are written and the build only carries it out. 039's
paragraph on a field declaring `initial` gains the sentence that a field
declaring `state_move` co-owns nothing when its last write was the move,
and that the report does not name it; 065's sentence on co-ownership
gains that such a value does not keep a record either. Both name this
record, and both carry the line under "How it came to be", proposed for
0.24.0. Neither record gains a case, a refusal or a sentence of its own:
the three acts and their preflights already answer through one predicate,
and this record changes what that predicate reads.

### The conformance rows

Six rows, in the suite, so they run on every engine; each asserts that its
preflight answers as the act does, and the acts answer as each other:

- `STATE_MOVE_DECLARED`, in `checks/purge.rs` beside `INITIAL_NO_OWNER`:
  the declaration is answered by `describe`, leaves the schema hash as it
  was, steps the declaration's `version`, and is refused with each of the
  four sentences where it cannot stand.
- `STATE_MOVE_PURGED`, in `checks/purge.rs`: the invoice of the opening —
  a source's record, a verb that marks it paid writing `status` and
  `paid_date` — is taken by `purge_source`, the report names no
  `PurgeMoved`, and the preflight named the same record.
- `STATE_MOVE_HAND_EDIT`, in `checks/purge.rs`: the same record with
  `paid_date` written by an author instead — through `upsert` and again
  through an accepted proposal — co-owns; the purge refuses it and its
  preflight says so.
- `STATE_MOVE_REMOVED`, in `checks/removed.rs`: `remove_values` on the
  source retires the record that holds only the move's values, keeps the
  one that holds an author's, and its preflight agrees.
- `STATE_MOVE_FORGOTTEN`, in `checks/forgotten.rs`: `forget_source` retires
  the record the move touched, and its preflight agrees.
- `STATE_MOVE_OTHER_VERB_WRITE`, in `checks/purge.rs`: a second verb of
  the type sets the `state_move` field outside the state change, and the
  record still goes with its source — the marker reads the procedure, not
  the move, so a verb's write that is not the move co-owns nothing either,
  and the report names nothing. The row pins the coarse answer the
  declaration carries.

### The build plan

Measured at this branch's base (`scripts/gates/core-size.sh`): product
114 998 of 115 000 (2 free), specification 202 995 of 203 000 (5 free).
What follows is a forecast, not a measurement: the code is not written,
so this record records no budget row and moves neither base, and decision
047 asks the build's own pull request for the measured lines, the cut it
made first and its row.

| Part | Lines |
|---|---|
| `FieldDecl.state_move`, its serde attributes and its doc | 5 |
| `InputWire`'s member and its mapping | 3 |
| `FieldDescription`'s member and its mapping | 4 |
| the four cases, the place, their sentences and the samples | 24 |
| the declaration's check in `declare.rs`, in `erasable`'s shape | 22 |
| the shared predicate, net of the two closures it replaces | 3 |
| the `moved` report's filter | 2 |
| owed wording: `PurgeMoved`'s doc and `log-format.md` | 6 |
| **product** | **69** |

That is 67 over the base before the cuts, so the build is likely to need
a row in `docs/budgets.md` under `docs/decisions/047-…`: **product
115 000 → 115 500**, the need rounded up to the next 500 — if, after its
cuts, it still exceeds the base. The row is the build's to record, with
its measured number.

**Cut first, with what each costs:** the two `initial` closures
(`purge.rs:165–169`, `remove.rs:603–607`) fold into the one predicate, about
10 lines out for 13 in; the declaration's check borrows `erasable`'s
structure (its `names` closure and its `on` vector) rather than restating
it.

| Part | Lines |
|---|---|
| unit tests in `tests.rs` files: serde shape, `shape_alone`, the wire, the check, the hash, the predicate | 130 |
| the six rows and their registration in `checks/mod.rs` | 500 |
| **specification** | **630** |

That is 625 over the base before the cuts, and the second row the build
is likely to need is **specification 203 000 → 204 000** — measured and
recorded there, not here. A row in the tree costs between 54 and 204
lines, measured over `checks/purge.rs` (1 794 lines, 33 rows),
`checks/forgotten.rs` (2 043, 10), `checks/removed.rs` (2 541, 20) and
`checks/erasable.rs` (1 015, 6); the six rows share one fixture — the
invoice's type, its source and its verbs, declared once — so they cost
about 500 rather than six times the average.

### Open for the operator

- The name: `state_move` is what the issue calls it. If the operator would
  rather the declaration read as what it does to the take-back, the name is
  theirs to set before the code lands.
- Whether a move that changes what a record is *for* should keep the record
  instead. This record says no: the issue's rule is that a take-back does
  not count a move as an author's value, and the invoice goes with its
  source. An operator who wants the invoice to outlive its source is asking
  for the value to become the author's record, which is another declaration
  and not this one.
- Whether the declaration should be refused on a type that keeps no
  per-field provenance rather than left inert, below. This record leaves it
  inert, because such a type's records already co-own nothing.
- Whether the design belongs to this line at all. Lakelands#1630 is
  labelled `after-0.23.0` and sits in milestone 0.24.0, and the tree is
  0.23.0 (v0.22.0 tagged): by the repository's rule, work labelled after
  the current line is not started early, and the triage ordered the design
  as the first step. If the operator would rather it wait for the 0.24.0
  line, nothing is built and this record waits with the issue.

## Why

`Origin` has one value for an author's write, and a verb's move and an
operator's hand-edit are both that value: `authored`. The distinction the
take-back needs — was this value chosen, or did the move put it there? — is
not in the record and cannot be recovered from it. The event carries it,
in `procedure` (`crates/lakelands-api/src/record.rs:56–59`), and the fold
keeps it per field. So the information is there; what is missing is the
ontology's word that a field's value is the move's, and that is a
declaration.

The alternative — treating every field a verb sets as the move's — was
rejected above: a verb's `set` says a field may be written by a verb, not
that the verb's write is a move. The alternative of narrowing the take-back
instead, so that a record a verb has touched is never taken, would keep
every record any verb has ever touched, which is most of them.

Two copies of the `initial` skip is the reason this record folds them: the
acts answer through `co_owned` and `holder_of`, and a third copy of the
question in either place would be the one that drifts. The build plan
forecasts 69 product lines and 630 spec lines before its cuts, 2 and 5 of
them free at this record's tree; it is a forecast, so the build measures
them against decision 047 and records its row there.

## What it is not

- It is not a transfer of the record to the author. The record stays the
  source's; the move's value is no author's, and the record goes with the
  source. What an author wrote directly is still the author's, and still
  keeps the record.
- It is not a mark on a record. Nothing the store writes records that a
  record was moved; the declaration is read at the act and the provenance
  of each field answers for it.
- It does not weaken decision 039 for a field that does not declare it, and
  it changes no answer for a lake that declares it nowhere.
- It is not `initial`. `initial` is declarative and unconditional — a field
  declaring it co-owns nothing, whoever wrote it last; this one reads
  provenance and applies only to a move's value. They may stand together,
  and the issue's `status` does.
- It is not `authority: verb`, which would make the hand-edit impossible
  (lakelands#1130).
- It is not a way to keep a record's value out of a source's erasure: a
  field declaring `state_move` that a source writes is refused the
  declaration, and `erasable` is untouched.
- On a type that keeps no per-field provenance (`TypeDecl.field_meta`,
  `ontology.rs:1356–1358`) it is inert: `meta` is absent, so no value
  co-owns on such a type, as today, and the declaration has nothing to
  read. That is also why leaving it inert is safe rather than silently
  stricter.
- It is not a new query language, and no model is called.
