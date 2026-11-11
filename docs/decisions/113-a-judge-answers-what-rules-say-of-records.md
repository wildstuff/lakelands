# 113 — A judge answers what the rules say of records, and writes nothing

Two questions a host asks of a lake have no door of their own. A host
about to import records wants to know, before it sends them, which an
ingest would admit, which it would quarantine and which it would refuse;
and a host or operator considering a rule wants to know what that rule
would say of the records already stored. Both are the store's to answer:
both are judged by the lake's one evaluator (023) over the lake's own
declarations, and neither may append anything.

The read doors cannot answer them. `find`'s `where` is checked at
`Place::Record`, which refuses `preceding()` (`Case::PrecedingHere`,
`api/src/expr/check.rs:113`) and evaluates `previous()` and `changed()`
as `Case::OnlyInWrite` outside a write (`core/src/expr/eval.rs:766`);
and a filter
answers records, never the name of the rule a record breaks nor the
refusal a write would give. `upsert_preflight` answers the first
question for one type's stored records through one write's decision,
but it takes a patch, names a single record and is the host's, with no
door (`api/src/catalog.rs:614-616`). So this record adds one generic,
read-only door, `judge`.

## What is decided

### One generic read-only door on the caller's contract, and no tool

**`Catalog` gains `judge`, beside `find` and `get`, and it appends
nothing whatever it answers.**

- The call is `async fn judge(&self, who: &Principal, ty: &TypeName,
  judge: Judge) -> Result<Reply<JudgeReport>>`. It sits with the generic
  doors, not with the per-type four: it is asked of a type's rules, and
  a per-type copy of it would be the same call four times (014).
- Its default body is `thin`'s (`api/src/catalog.rs:386-397`): the
  arguments are dropped and the answer is
  `Error::unbound(Seam::Operation { op: "judge" })`. An implementor of the trait that does not judge compiles
  and says so, and a lake reached over a network that has no route for
  it answers unbound, as a remote lake answers `upsert_preflight`
  today. On the crate's trait it is an addition, not a breaking change:
  the method has a default body, so every implementor compiles
  unchanged. It is not silent to a WIT caller. 034 holds that the WIT
  mirrors the crate, and the drift test names every door of the
  `Catalog` as a function of the package (`api/tests/wit.rs:191`);
  `wit/store.wit`'s `catalog` gains `judge`, its call and answer types
  join `wit/types.wit`, and the `provider` world
  (`wit/worlds.wit:11`) exports the interface that carries it. A
  component provider meets a new function and must implement it, and a
  change a WIT caller sees is a new package version (034), so the
  package version moves in the same build. The Rust default body keeps
  an embedding host compiling; the package version keeps a component
  provider from meeting the change unannounced.
- It has no tool. The names a model sees are produced only by
  `lakelands_api::tools::tools` (`api/src/tools.rs:382`), and this record
  adds no name there, so the `single-catalog` gate and every model's tool
  list are unchanged. A preview before an import and a check of a rule
  not yet declared are a host's or an operator's questions, asked with
  what the host already holds; they are not a model's. The doors of the
  `Catalog` with no tool of their own become seven (014, amended).
- One type per call, named once as `find` names it: every key and every
  candidate judged is of `ty`. There is no cross-type rule in the
  contract, so a call that mixed types could answer no rule of either.

### Two subjects, one per call

`Judge.subject` is one of:

- **`Subject::Keys(Vec<RecordKey>)`** — stored records, read at
  `Judge.at: ReadAt` (`api/src/read.rs:42`), so `as_of`, `valid_at` and
  `clock` are the read's axes and clock (052). A key the type does not
  hold is refused `not_found` as `get` refuses it (`Missing::Record`),
  and no record is answered.
- **`Subject::Candidates(Vec<SourceRecord>)`** — records not stored
  (`api/src/record.rs:173-176`; a source record carries its own `ty` and
  its key), judged exactly as the ingest window judges them
  (`store/src/ingest.rs:336 fn judge`): `Door::Source`, every rule that
  admits at ingest judged, `admit` rules reported as well as refused
  ones, `preceding()` resolved from the stored series, `seen_at` the
  record's own, `known_at` the judge's clock. A candidate whose own
  `ty` is not the call's is refused whole `invalid` with
  `Case::CandidateOfAnotherType { named, carried }`, naming both, as a
  call that mixed them could answer no rule of either; nothing is
  judged for any candidate of that call.

Never both in one call. A stored record is judged at the read's
position over its own stored meta; a candidate is judged at the write's
clock over values that are not in the lake. One answer that held both
would have to say which instant and which scope each record was judged
at, and a caller that wants both judgements asks twice.

### The rules: those the caller is shown, or one it proposes

`Judge.rules` is one of:

- **`Rules::Declared { names: Option<Vec<ConstraintName>> }`** — every
  constraint of the type the caller is shown, or the ones it names. What
  is shown is `describe_type`'s set: the constraints whose fields its
  reader reads (`api/src/describe.rs:151-154`, lakelands#1119). A named
  rule the type does not declare, or declares and this caller is not
  shown, is
  refused `not_found` with a new `Missing::Constraint { name }` — the
  sentence names the caller's own word and no field (109).
- **`Rules::Proposed { name: ConstraintName, holds: Condition }`** — one
  condition the caller is considering, checked at `Place::Constraint`
  (`api/src/expr/check.rs:16`), the place a declaration's condition is
  checked at. That place check is not the whole of what a declaration
  may read: `condition()` and `constraint_reach` narrow a declaration
  further, and the section below states what a proposed rule is and is
  not held to. It is answered under the name the caller gave it, so a
  sample check says what a declaration under that name would say.
  Nothing lands: declaring a rule is `declare_type`'s and changing one
  is `migrate`'s.

### A stored record is judged at the read's clock, and what only a write decides is named

**The judge is no walk. It names the conjuncts only a write decides,
and judges every other conjunct at the read's clock, for both rule
forms.**

- 059 § Only the write judges its clock and the record before it holds
  that the write decides its own instant and the record before it, and
  that no later reader answers them in the write's place. The judge
  does not take the walk's scope for a stored record: `Walked`'s scope
  sets `before` to the record itself (`store/src/writes.rs:269-278`),
  so a rule reading `previous()` would be judged against a state no
  write ever had — "a walk is no transition" (`store/src/writes.rs:164`).
  That scope is the walk's because a walk asks whether it must refuse;
  the judge asks what the rule says, and answers what a read can.
- Over a stored record, a conjunct calling `preceding()`, `previous()`,
  `changed()`, `retired()` or `created()` is answered `unjudged`
  `TheWriteAlone`, once for the rule and for every record of the call,
  and every other conjunct is judged. The evaluator refuses those five
  at a read's scope — `Case::OnlyInWrite`
  (`core/src/expr/eval.rs:766-783`), `Case::RetiredOnlyInGuard` and
  `Case::CreatedOnlyInGuard` (`core/src/expr/eval.rs:804-812`) — and
  the judge reports each as `TheWriteAlone` rather than as an
  unevaluable case: the part is not malformed, the write that wrote the
  record decided it.
- `now()` and `today()` are not in that set. They are the read's clock,
  `Judge.at`'s `clock` (052), and they are judged — for a declared rule
  and for a proposed rule alike. The ruled-out read judges `today()` at
  the read's clock too (`core/src/conformance/checks/read_clock.rs:17`); 059
  binds the write, not a read that decides nothing. One rule for both
  forms is what keeps `JUDGE_READS_AT_THE_READS_CLOCK` green for each
  of them. It is also why the build does not use `Expr::walked()`
  (`api/src/expr/mod.rs:528`): that function drops `now()` and
  `today()` together with `preceding()`, which is right for a walk's
  clock and wrong for a read's.
- `known_at()` and `seen_at()` read the record's own meta
  (`core/src/expr/eval.rs:758-763`), not the judge's clock.
- `in_force()` is refused by the check at this place: the check allows
  it only in a computed field at the read and in a trigger's guard, not
  in a constraint (`api/src/expr/check.rs:115-117`), so a rule calling
  it is refused whole `invalid`, not judged over a record.
- A candidate is judged as the ingest window judges it. The write's
  scope supplies the record before it, `preceding()` resolved from the
  stored series, `now()` at the window's clock, and `retired()` and
  `created()` as the write leaves them
  (`store/src/writes/record.rs:524-533`), so none of the five is
  `TheWriteAlone` for a candidate. The caveat 059 states of the
  preflight applies unchanged: a rule reading the clock may judge an
  instant before the write's, so a candidate is judged as the window
  judges it, not as the write will.
- A conjunct the evaluator cannot evaluate is `unjudged`
  `Unevaluable { index, case }`, naming the record and its case, and
  never a breaker. The walk's own judge writes
  `expr::holds(..).unwrap_or(false)` (`store/src/writes.rs:279`), which
  is sound for a walk asking whether it must refuse and wrong for a
  judge; the build does not reuse that line.

### What a constraint may read, and what a proposed rule may

`Rules::Proposed` is checked at `Place::Constraint`, the place a
declaration's condition is checked at, but that check is not the whole
of what a declaration may read. A declaration is narrowed twice more:
`condition()` refuses `in_set()` for every place
(`store/src/declare.rs:397-404`, `Case::InSetHere`), and
`constraint_reach` refuses a path longer than one segment, `exists()`,
a setting, a computed-at-read field and a computed-outside field
(`api/src/ontology.rs:883`, `Case::ConstraintReadsBeyond`). A proposed
rule is held to the place check and judged; it is not refused for what
a declaration's reach would refuse, because the question is what the
rule says of the records, not whether it may be declared. The two
functions the issue names are answered as the tree answers them:

- **`in_set()`** is refused for every place when the condition is
  typed (`store/src/declare.rs:397-404`), though the place check lets a
  quoted one through (`api/src/expr/check.rs:118-120`), so a proposed
  rule calling it is refused whole `invalid` with `Case::InSetHere`,
  and no record is judged.
- **`exists()`** passes the place check in a constraint, which is a
  guard (`api/src/expr/check.rs:93-94`, :151-152), and a declared
  constraint can never carry it because `constraint_reach` refuses it.
  A proposed rule can, and no scope the judge holds answers it:
  `Scope::exists` defaults to `None` (`core/src/expr/eval.rs:154-156`),
  and the only scope that supplies an across is a verb's
  (`store/src/writes/across.rs:875-877`). The evaluator answers
  `Case::ExistsHere` (`core/src/expr/eval.rs:816-820`), so the judge
  reports `unjudged` `Unevaluable { case: ExistsHere }` — for a stored
  record and for a candidate alike, because a candidate's condition is
  judged with the same write scope, which supplies no across
  (`store/src/writes/record.rs:524-533`). The judge is no verb and
  never invents one.

### The answer is the write's own refusal, where the rule is judged whole

`JudgeReport { records: Vec<JudgedRecord>, unjudged: Vec<Unjudged> }`,
with:

- `JudgedRecord { index: u32, key: RecordKey, refusals: Vec<Refusal> }`
  — every rule the record breaks in the part the judge judged, each as
  the refusal a write gives: `Refusal::Constraint` of
  `Case::ConstraintBroken { constraint, ty, key }`, whose sentence is
  "`{ty}` `{key}` would break the constraint `{constraint}`"
  (`api/src/error/case.rs:1780`), unchanged. Where the rule is judged
  whole, the sentence is the write's word for word, and a host that
  compares the judge's answer with a later write's refusal compares the
  same text, so a preview is not a second vocabulary that can drift.
  Where it is not judged whole, the breaker is still the write's, and
  the part no read judges is named beside it in `unjudged` — a caller
  reads both together, never the breaker alone.
- A candidate the write would refuse before it reaches the rules
  answers that refusal alone, as the write stops there; the rules are
  not judged for it.
- A rule that admits at ingest (`AtIngest::Admit`,
  `api/src/ontology.rs:1409`) is judged and reported too, as a walk
  reports it (`Walked::judges`, `store/src/writes.rs:240`): a host
  previewing what would be admitted broken is told before it sends.
- Records are answered in subject order, each with its index, so a
  caller lines the answer up with what it sent; a record that breaks
  nothing answers an empty list.
- `Unjudged { constraint: ConstraintName, why: UnjudgedWhy }` names a
  rule not judged whole, once per rule and reason. `UnjudgedWhy` is
  `TheWriteAlone` (the rule reads what only a write decides over a
  stored record — `preceding()`, `previous()`, `changed()`, `retired()`
  or `created()` — and no read judges that part: 059), the same for
  every record of the call, or `Unevaluable { index: u32, case: Case }`
  (the evaluator answered a case over the record at that index) — so a
  rule that cannot be evaluated over one record is named with the
  record, and a caller never reads silence about it as a pass. A rule
  can appear in `unjudged` and in `refusals` at once: it broke the part
  the judge judged and its other part no read judges. A caller that
  sees no breaker and an `unjudged` entry knows the rule may still
  break at a write.
- No field's value appears anywhere in the answer. A breaker names the
  rule, the type and the key, as `ConstraintBroken` does, and the
  unjudged entry names the rule and a reason; that is the whole of it.
- A call that cannot be answered as asked is refused whole — a key not
  there, a rule not declared or not shown, a condition that does not
  check, a candidate of another type, more subjects than the bound.
  Nothing partial is answered, as no read answers part of a refused
  call.

### Permissions: a rule the caller may not read is not judged and not named

- The judge judges the rules `describe_type` lists to the caller, and no
  others. A declared rule reading a field the caller does not read is
  not among them: it is neither judged nor named, because naming it
  would tell the caller what it is kept from (105, 109). The judge's
  silence about it is the silence `describe_type` already keeps.
- A proposed condition that reads a field the caller may not read is
  refused `not_granted` naming the field, as a read's probe is (105):
  the caller named the field itself, so the refusal tells it nothing it
  did not bring.
- A candidate carries values the caller already holds, so judging it
  tells the caller nothing it did not send; the answer still names none
  of them, and no stored value is told through a candidate's breaker.

### Bounded, and never cut

- A call carries at most `limits.max_limit` subjects
  (`api/src/location.rs:297`; 1000 by default), keys and candidates counted
  together. More is refused `too_large` on the field `records`, and
  never cut, so a host that wants a whole type pages it with `find` and
  judges a page at a time.
- That refusal needs one arm in the error's own display
  (`api/src/error.rs:689-691`), because the existing catch-all for a
  named field says "the set's memberships would change". One arm, not a new
  `Refusal`.
- The judge selects nothing: it carries no `where`, no `order`, no
  `limit`, no cursor, and no whole-type sweep. Choosing records is
  `find`'s question (018's pattern, 017's grammar) and judging them is
  the judge's, so this is not a second query language.

### It is not the preflight and not the ruled-out read

- `upsert_preflight` stays as it is. It answers one write's whole
  decision — the head it reads, the writer's lease and state, the
  idempotency key, a patch applied to the live record — and it is the
  host's, with no door. The judge is narrower and is on the contract: it
  reads records as a read does, takes no patch and no change, and
  answers rules rather than one write's verdict. Neither replaces the
  other, and this record does not make the preflight a door.
- The ruled-out read stays as it is (052). A `find` whose `where` reads
  `today()` answers the keys a condition rules out at the read's clock,
  and that remains the cheapest way to ask it of the caller's own
  condition. The judge adds what a filter cannot carry: a declared
  rule by the name the lake knows it by, with the refusal code a write
  gives, judged in the part a read judges; and a proposed condition
  whose `preceding()` a filter refuses outright
  (`api/src/expr/check.rs:113`).
- A judge is not a write, not a proposal and not a migration. It
  appends nothing, so the log cannot tell it happened, and no position
  moves because of it.

### The build, and what it costs

- Measured on develop when this was written:
  `scripts/gates/core-size.sh` is green at product 114998 of a base and
  ceiling of 115000, so two product lines are free, and specification
  202995 of 203000, so five are; `scripts/gates/seam-width.sh` is green
  at 111 of 111 methods, and `judge` costs it nothing, because the seam
  counts `LakeStore` and the traits in `engine.rs` and not the `Catalog`
  (`api/src/catalog.rs:468`).
- The build is far more than two lines. What it adds: the method, its
  unbound default and the call and answer types, about 120 lines with
  their docs and serde attributes in a new `judge.rs` beside
  `catalog.rs`; one `TooLarge` arm for the field `records`, with the
  variant's doc; `Missing::Constraint` with its sentence;
  `Case::CandidateOfAnotherType { named, carried }` with its sentence;
  a `judge` implementation over the store's read path that calls the
  one evaluator with two scopes — the stored record's and the ingest
  window's — and splits a rule with `Expr::conjuncts()`
  (`api/src/expr/mod.rs:476`) and `Expr::calls()`
  (`api/src/expr/mod.rs:484`); the `Catalog` trait's sentence naming
  the generic doors; and the rows below, with their tests in
  `tests.rs`. It does not reuse `Expr::walked()`
  (`api/src/expr/mod.rs:528`), which drops the clock with
  `preceding()`; nor `Walked::judge` (`store/src/writes.rs:254`),
  which takes a `(was, is)` pair and judges only the fields a write
  moved; nor the ingest window (`store/src/ingest.rs:336`), which is a
  window loop and not a judgement of keys. Sketched, it is roughly 300
  to 400 product and 220 to 320 specification lines; the build reports
  what it measures, and its row is set to that count.
- Either way it cannot land inside the base it inherits, so the build
  needs two rows in `docs/budgets.md` in 047's form: the product to
  115500 and the specification to 203500, each base the build's
  measured count rounded up to the next 500, as every row there is.
  047's first condition is the one to state — the room is for a contract
  a host depends on, named in its issue (lakelands#1670), not for
  tidying — and its second holds: the build cuts first, by judging
  with the one evaluator and naming what it cannot answer, so there is
  one evaluator, one split and one refusal vocabulary.
- This change adds no code and no row. No door is implemented here: the
  gates are unchanged, the two bases are untouched, and the rows are the
  build's to carry.

### The rows the suite gains

Every row below runs on every engine, as the suite does, and each is a
claim of the contract rather than of one engine:

- `JUDGE_WRITES_NOTHING` — a judge of stored keys, of candidates, and
  of a record that breaks a refusing rule appends nothing: the head, the
  log's length, every record's `known_at`, the source's offsets and the
  quarantined set are what they were, and a write of the same record is
  still refused.
- `JUDGE_SAYS_WHAT_THE_WRITE_SAYS` — for a record and a declared rule
  the judge judges whole and that refuses, the judge's refusal is the
  write's, word for word: the same `Constraint` refusal and the same
  `ConstraintBroken` sentence, naming the constraint, the type and the
  key; and where the rule is not judged whole, the breaker it reports
  for the part it did judge is that same refusal, with the unjudged
  part named beside it. An admitting rule the record breaks is reported
  too, as a walk reports it.
- `JUDGE_NAMES_WHAT_ONLY_A_WRITE_DECIDES` — over stored records a rule
  reading `preceding()`, `previous()`, `changed()`, `retired()` or
  `created()` is answered `unjudged` `TheWriteAlone`, once for the rule
  and the same for every record of the call, and never as a pass, its
  other conjuncts still judged; `known_at()` and `seen_at()` in a rule
  read the record's own meta, not the judge's clock; a candidate is
  judged as the ingest window judges it, so none of the five is
  `TheWriteAlone` for it; a rule calling `in_force()` over either
  subject is refused whole `invalid`; and a conjunct the evaluator
  fails is `unjudged` `Unevaluable`, never a breaker.
- `JUDGE_JUDGES_WHAT_THE_WRITE_JUDGES` — a candidate for a stored
  record is judged by the rules its write would judge, the window's own
  step deciding: a rule over a field the candidate does not send is not
  judged for it, though the stored record breaks it; a candidate that
  leaves the record as it stands is refused by none.
- `JUDGE_READS_AT_THE_READS_CLOCK` — a declared rule and a proposed
  rule reading `today()` or `now()` over stored keys each answer, at
  each of the read's clocks, exactly the keys `TODAY_RULES_OUT`
  answers for the same condition: one clock rule for both forms.
- `JUDGE_NAMES_WHAT_NO_SCOPE_ANSWERS` — a proposed rule calling
  `in_set()` is refused whole `invalid` with `Case::InSetHere`, and
  no record is judged; a proposed rule calling `exists()` over a stored
  key and over a candidate alike answers `unjudged`
  `Unevaluable { case: ExistsHere }`, named with each record and never
  as a pass; and a declared rule can carry neither, `condition()` and
  `constraint_reach` refusing them at declaration.
- `JUDGE_ANSWERS_NO_FIELD` — a declared rule the caller is not shown is
  neither judged nor named; a proposed rule reading a masked field is
  refused `not_granted` naming the field; and no stored value appears in
  any answer, a breaker naming the rule, the type and the key and
  nothing more.
- `JUDGE_IS_BOUNDED` — more subjects than `limits.max_limit` is refused
  `too_large` on `records`, never cut, keys and candidates counted
  together; a page of a whole-type judgement is judged whole.
- `JUDGE_JUDGES_ONE_TYPE` — a key the named type does not hold is
  refused `not_found` and nothing is answered; a rule the type does not
  declare is refused `not_found`; a candidate whose `ty` is not the
  call's is refused whole `invalid` with
  `Case::CandidateOfAnotherType { named, carried }`, nothing judged for
  any candidate of that call; and no call judges two types at once.
- `JUDGE_IS_NO_TOOL` — the catalog a principal receives names no tool
  that judges, and `single-catalog` holds over the whole list.

## Why

The store is the only party that can answer either question without
guessing. It holds the declarations the rules come from, the projection
the records are read from, and the one evaluator that decides them
(023); a host that restated a rule in its own words would answer a
different rule as soon as the declaration changed, which is the reason
`upsert_preflight` exists at all. This record gives that reason a door a
caller can hold, for records in hand and for a rule not yet declared.

The answer is the write's own refusal because the caller's purpose is
to say to its operator what will happen, or what would happen, in the
words the store will use. A boolean, or a parallel set of codes the
host translates, would drift from the write's sentences and from the
rule names the lake declares. Where the judge judges a rule whole,
answering `Refusal` values the writer already gives makes the preview
exact by construction; where it does not, the answer names the part no
read can judge beside the breaker, so the preview is never read as more
than the read can say.

It is a read because it decides nothing. Every guarantee a read gives
applies unchanged: the clock it is read at (052), the principal it is
read as, and the fields that principal may read (105). Nothing it
answers may be acted on as though it had been decided: a rule accepted
after a sample check, or a record imported after a preview, is judged
again by the call that lands it.

It is generic because the question is of a type's rules, and a type is
named once at a generic door; the per-type four exist because they act
on one type's records, and a judge acts on none.

## What it is not

- It is not an ingest preflight. It does not decide delivery, a window's
  halting, quarantine's admission or a window's counts; a candidate that
  would be quarantined is judged by the rules a write judges, not by
  085's admission.
- It is not a proposal's telling. A proposal's plan is judged by
  `propose` under 062's rules; the judge answers a rule against stored
  records and lands neither.
- It is not an authorization to write, and it does not answer for
  another writer. A write's grants (012, 106) are the write's to check;
  the judge answers what the rule says of the record, so a caller that
  may read a record but not write it can still be told the rule.
- It does not judge what only a write decides over a stored record:
  `preceding()`, `previous()`, `changed()`, `retired()` and `created()`
  are named `TheWriteAlone` there (059), and it never reports an
  unjudged part as a pass. It does not answer a stored record at the
  write's clock: `now()` and `today()` are the read's clock (052).
- It does not offer a tool, and it does not become one (014).
- It does not follow a reference: a rule reading a reference field is
  judged on the stored link value, not on the record it points at.
- It does not answer another type's records or rules, and it does not
  select: it judges the subjects it is given.

## How it came to be

- Added at lakelands#1670: one generic read-only door, `judge`, its two
  subjects, its two rule forms, the split that names what only a write
  decides over a stored record and judges the rest at the read's clock
  (one clock rule for both forms), the refusal-worded answer where a
  rule is judged whole, and its bound. `judge` joined the generic doors
  of 014.
