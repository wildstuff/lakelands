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
`expr/check.rs:113`) and evaluates `previous()` and `changed()` as
`Case::OnlyInWrite` outside a write (`expr/eval.rs:766`); and a filter
answers records, never the name of the rule a record breaks nor the
refusal a write would give. `upsert_preflight` answers the first
question for one type's stored records through one write's decision,
but it takes a patch, names a single record and is the host's, with no
door (`catalog.rs:615`). So this record adds one generic, read-only
door, `judge`.

## What is decided

### One generic read-only door on the caller's contract, and no tool

**`Catalog` gains `judge`, beside `find` and `get`, and it appends
nothing whatever it answers.**

- The call is `async fn judge(&self, who: &Principal, ty: &TypeName,
  judge: Judge) -> Result<Reply<JudgeReport>>`. It sits with the generic
  doors, not with the per-type four: it is asked of a type's rules, and
  a per-type copy of it would be the same call four times (014).
- Its default body is `thin`'s (`catalog.rs:385-397`): the arguments
  are dropped and the answer is `Error::unbound(Seam::Operation { op:
  "judge" })`. An implementor of the trait that does not judge compiles
  and says so, and a lake reached over a network that has no route for
  it answers unbound, as a remote lake answers `upsert_preflight`
  today. Adding the method is an addition to the contract, not a
  breaking change.
- It has no tool. The names a model sees are produced only by
  `lakelands_api::tools::tools` (`tools.rs:382`), and this record adds
  no name there, so the `single-catalog` gate and every model's tool
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
  `Judge.at: ReadAt` (`read.rs:42`), so `as_of`, `valid_at` and `clock`
  are the read's axes and clock (052). A key the type does not hold is
  refused `not_found` as `get` refuses it (`Missing::Record`), and no
  record is answered.
- **`Subject::Candidates(Vec<SourceRecord>)`** — records not stored
  (`record.rs:173`; a source record carries its key, not an option of
  it), judged exactly as the ingest window judges them
  (`store/src/ingest.rs:336 fn judge`): `Door::Source`, every rule that
  admits at ingest judged, `admit` rules reported as well as refused
  ones, `preceding()` resolved from the stored series, `seen_at` the
  record's own, `known_at` the judge's clock.

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
  reader reads (`describe.rs:151-154`, lakelands#1119). A named rule the
  type does not declare, or declares and this caller is not shown, is
  refused `not_found` with a new `Missing::Constraint { name }` — the
  sentence names the caller's own word and no field (109).
- **`Rules::Proposed { name: ConstraintName, holds: Condition }`** — one
  condition the caller is considering, checked at `Place::Constraint`
  (`expr/check.rs:16`), the place a declaration's condition is checked
  at, so it may read what a constraint may read and is refused as
  invalid where it does not check. It is answered under the name the
  caller gave it, so a sample check says what a declaration under that
  name would say. Nothing lands: declaring a rule is `declare_type`'s
  and changing one is `migrate`'s.

### What a stored record is judged on

**A rule over stored records is judged in the part a walk judges, and
every part it does not judge is named, never passed.**

- 059 § Only the write judges its clock and the record before it
  holds that the clock and `preceding()` are decided by the write that
  wrote the record and judged again by nobody. `Expr::walked()`
  (`expr/mod.rs:528`) keeps the conjuncts that call none of `now()`,
  `today()` and `preceding()`, and `Walked` judges exactly those
  (`store/src/writes.rs:254`). The judge over stored records uses the
  same split: it judges the walkable conjuncts and reports the rule with
  its unjudged part.
- The walk's scope is the judge's for a stored record: `before` is the
  record itself, so `previous()` is the record and `changed()` is false
  (writes.rs:269-278; "a walk is no transition",
  writes.rs:164), `retired()` and
  `created()` are false, `in_force()` is refused by the check at this
  place, and `known_at()` and `seen_at()` read the record's own meta
  (`expr/eval.rs:759`), not the judge's clock.
- A proposed rule is judged the same way and reported the same way: a
  conjunct calling `preceding()` is unjudged (the walk supplies none),
  and the rest is judged at the read's clock.
- The read's clock is the judge's `now()` and `today()`: `Judge.at`'s
  `clock` (052). So a proposed rule reading `today()` over stored
  records answers the keys it rules out at the read's clock, as the
  ruled-out read does (052; `conformance/checks/read_clock.rs:17`).
- For candidates, `now()` is the ingest window's clock, and the caveat
  059 states of the preflight applies unchanged: a rule reading the
  clock may judge an instant before the write's, so a candidate is
  judged as the window judges it, not as the write will.
- A conjunct the evaluator cannot evaluate is unjudged, with its case,
  and never a breaker. The walk's own judge writes
  `expr::holds(..).unwrap_or(false)` (`writes.rs:279`), which is sound
  for a walk asking whether it must refuse and wrong for a judge: the
  build surfaces the evaluator's answer as `UnjudgedWhy::Unevaluable`,
  naming the record and its case, here and does not reuse that line.

### The answer is the write's own refusals, word for word

`JudgeReport { records: Vec<JudgedRecord>, unjudged: Vec<Unjudged> }`,
with:

- `JudgedRecord { index: u32, key: RecordKey, refusals: Vec<Refusal> }`
  — every rule the record breaks, each as the refusal a write gives:
  `Refusal::Constraint` of `Case::ConstraintBroken { constraint, ty,
  key }`, whose sentence is "`{ty}` `{key}` would break the constraint
  `{constraint}`" (`error/case.rs:1780`), unchanged. A host that
  compares the judge's answer with a later write's refusal compares the
  same text, so a preview is not a second vocabulary that can drift.
- A candidate the write would refuse before it reaches the rules
  answers that refusal alone, as the write stops there; the rules are
  not judged for it.
- A rule that admits at ingest (`AtIngest::Admit`,
  `ontology.rs:1409`) is judged and reported too, as a walk reports it
  (`Walked::judges`, writes.rs:240): a host previewing what would be
  admitted broken is told before it sends.
- Records are answered in subject order, each with its index, so a
  caller lines the answer up with what it sent; a record that breaks
  nothing answers an empty list.
- `Unjudged { constraint: ConstraintName, why: UnjudgedWhy }` names a
  rule not judged whole, once per rule and reason. `UnjudgedWhy` is
  `TheWriteAlone` (the rule reads the write's clock or `preceding()`,
  and no read judges that part: 059), the same for every record of the
  call, or `Unevaluable { index: u32, case: Case }` (the evaluator
  answered a case over the record at that index) — so a rule that cannot
  be evaluated over one record is named with the record, and a caller
  never reads silence about it as a pass. A rule can appear in
  `unjudged` and in `refusals` at once: it broke the part a walk judges
  and its other part no read judges. A caller that sees no breaker and
  an `unjudged` entry knows the rule may still break at a write.
- No field's value appears anywhere in the answer. A breaker names the
  rule, the type and the key, as `ConstraintBroken` does, and the
  unjudged entry names the rule and a reason; that is the whole of it.
- A call that cannot be answered as asked is refused whole — a key not
  there, a rule not declared or not shown, a condition that does not
  check, more subjects than the bound. Nothing partial is answered, as
  no read answers part of a refused call.

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
  (`api/location.rs:297`; 1000 by default), keys and candidates counted
  together. More is refused `too_large` on the field `records`, and
  never cut, so a host that wants a whole type pages it with `find` and
  judges a page at a time.
- That refusal needs one arm in the error's own display
  (`error.rs:666-690`), because the existing catch-all for a named field
  says "the set's memberships would change". One arm, not a new
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
  gives, judged in the part a walk judges; and a proposed condition
  whose `preceding()` a filter refuses outright (`expr/check.rs:113`).
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
  (`catalog.rs:468`).
- The build is far more than two lines. What it adds: the method, its
  unbound default and the call and answer types, about 120 lines with
  their docs and serde attributes in a new `judge.rs` beside
  `catalog.rs`; one `TooLarge` arm for the field `records`, with the
  variant's doc; `Missing::Constraint` with its sentence; a `judge`
  implementation over the store's read path, reusing `Walked`'s split
  (writes.rs:254) and the ingest window's `judge` (ingest.rs:336) rather
  than a second evaluator; the `Catalog` trait's sentence naming the
  generic doors; and the rows below, with their tests in `tests.rs`.
  Sketched, it is roughly 300 to 400 product and 200 to 300
  specification lines; the build reports what it measures, and its row
  is set to that count.
- Either way it cannot land inside the base it inherits, so the build
  needs two rows in `docs/budgets.md` in 047's form: the product to
  115500 and the specification to 203500, each base the build's
  measured count rounded up to the next 500, as every row there is.
  047's first condition is the one to state — the room is for a contract
  a host depends on, named in its issue (lakelands#1670), not for
  tidying — and its second holds: the build cuts first, by reusing the
  walk's split and the window's judgement, so there is no second
  evaluator, no second rule-reading and no second refusal vocabulary.
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
  that refuses, the judge's refusal is the write's, word for word: the
  same `Constraint` refusal and the same `ConstraintBroken` sentence,
  naming the constraint, the type and the key; an admitting rule the
  record breaks is reported too, as a walk reports it.
- `JUDGE_JUDGES_AS_A_WALK` — over stored records a rule reading `now()`,
  `today()` or `preceding()` is answered `unjudged` `TheWriteAlone` and
  never as a pass, its walkable part still judged; `known_at()` and
  `seen_at()` in a rule read the record's own meta, not the judge's
  clock; a proposed rule reading `preceding()` is unjudged with its
  other part judged; and a conjunct the evaluator fails is
  `unjudged` `Unevaluable`, never a breaker.
- `JUDGE_READS_AT_THE_READS_CLOCK` — a proposed rule reading `today()`
  over stored keys answers, at each of the read's clocks, exactly the
  keys `TODAY_RULES_OUT` answers for the same condition.
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
  declare is refused `not_found`; and no call judges two types at once.
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

The answer is the write's own refusals because the caller's purpose is
to say to its operator what will happen, or what would happen, in the
words the store will use. A boolean, or a parallel set of codes the
host translates, would drift from the write's sentences and from the
rule names the lake declares. Answering `Refusal` values the writer
already gives makes the preview exact by construction.

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
- It does not judge the write's clock or `preceding()` over stored
  records (059), and it never reports an unjudged part as a pass.
- It does not offer a tool, and it does not become one (014).
- It does not follow a reference: a rule reading a reference field is
  judged on the stored link value, not on the record it points at.
- It does not answer another type's records or rules, and it does not
  select: it judges the subjects it is given.

## How it came to be

- Added at lakelands#1670: one generic read-only door, `judge`, its two
  subjects, its two rule forms, the walk's split over stored records,
  the refusal-worded answer and its bound. `judge` joined the generic
  doors of 014.
