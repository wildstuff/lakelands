# 075 — A read may cross lakes, and each lake answers its part

## What is decided

**Paused** on lakelands#1400 (2026-10-25, by the operator). Nothing of
it is built, and no issue builds it while it is paused:
`docs/decisions/042-water-crosses-only-as-a-mirror.md` stands as
written and nothing reads across lakes.

No host case was found for one grouped read over several lakes. A
consolidation is a sum of each lake's own answer, which the host puts
together. A host that lets one tenant read another's data does so
through the other lake's own principal and its grants
(`docs/decisions/006-grants-not-roles.md`,
`docs/decisions/051-a-grant-may-admit-only-some-records.md`), or keeps
what it must hold of the other lake as a mirror (042). The rules below
are what a read across lakes would keep, should a host case bring it
back; they bind nothing until a record takes it up again.

### A read across lakes is one read over several lakes, outside every one of them

A host that keeps one lake per company or per client asks one read of
several lakes: one type name, one filter, one order, one page, or one
aggregate, and the names of the lakes. The store answers it by asking
each lake for its part through that lake's own doors and putting the
parts together in its core. No lake reads another, and no lake holds
anything of another. The answer is kept nowhere. Like an export
(`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`),
the read is a function over the lakes' stores, beside them, and costs
no method on the engine seam.

The first step is `find` across lakes: its rows, its sort and page, its
`total`, and its aggregates. A walk, a pattern and a search are not part
of it (below).

### Each lake's part is read by that lake's principal, under that lake's grants

A principal names one lake, and holds no grant in any other
(`docs/decisions/006-grants-not-roles.md`). That stays. The host's
`PrincipalResolver` already resolves a credential in one lake at a time;
a read across lakes resolves the caller's credential once in each lake
it names, and each lake's part is read by the principal that lake
answered, exactly as a read of that lake alone would be: its masks, its
`only`, its retention holds, its renames followed, its probes. Nothing
widens a grant, and no principal is given several lakes.

- **A lake the caller holds nothing in refuses the whole read**, naming
  the lake, as the resolver refuses it. A consolidated sum without one
  company is a wrong answer that looks right; the caller drops the lake
  from the read or is given a principal there.
- **The type must be the same type in every lake it names**, compared as
  the store already compares shape. Its schema hash
  (`docs/decisions/007-schema-hash-covers-shape.md`) is the same in
  every lake; the hash covers its fields with their kinds and the names
  of their value types, its key and, for a view, its sources,
  conditions and parameters. A value type has no hash of its own (007), so every value
  type a field names is compared as declared: its kind, constraint,
  `one_of`, format and scale, all but its description and annotations
  (`docs/decisions/011-storage-kind-and-value-type.md`). A lake where
  the type is not shown refuses the whole read as a read of that lake
  alone would, naming the lake; a lake whose type or one of whose value
  types differs refuses it naming the lake and the type or the value
  type. Names alone are never compared: `money` kept at two decimal
  places in one company and as a number in another would be summed
  together.
- **What the store does not read, it does not compare.** A unit kept in
  a value type's annotations (`acme.unit: "EUR"`) is the host's, and the
  store never reads it (007): two lakes whose `money` differs only
  there are the same to the read, which sums euros with dollars. A host
  whose lakes keep different currencies reads them apart, or keeps the
  currency in a field and groups by it.
- **The caller is shown the same fields in every lake.** A `find`
  answers every field its reader is shown, with no choice of fields, so
  a field the caller's principal is shown in one lake and not in
  another refuses the whole read, naming the lake and the field. A
  field masked in a lake is shown there, masked (below). Nothing is
  answered as absent from one lake and present in another.
- **A value masked in its lake is masked in the answer.** A row carries
  what its own lake's principal reads of it, and nothing more.

### The answer carries one position per lake, and a repeat at those positions answers the same

Every answer carries its position
(`docs/decisions/009-every-response-carries-its-position.md`). The
positions of two lakes are two logs' coordinates and are never compared,
so an answer across lakes carries one position for each lake it read,
and each part stands at its own:

- **By default, each lake at its head** as that lake's part was first
  read. The parts are not one instant: a write to one lake after the
  read began and before its part was read is in the answer, and one to
  another lake at the same moment may not be. No cut across lakes is
  promised.
- **Pinned per lake.** A read may name an `as_of` for each lake, and a
  cursor carries the position of every lake, so every page of one read
  stands at the same positions, and a repeat at them by the same
  principals answers the same, bit for bit, on every engine, save what
  a hold placed since withholds (below).
- **Pinned to an instant.** A read may name one `known_at` instead: each
  lake is read at the last position whose event was known at or before
  it (`Catalog::position_at`, which needs no new engine question, since
  `known_at` never decreases along a log). That is the nearest a set of
  logs comes to one moment, and it is exact only as far as the lakes'
  writers share one clock. A lake with no event at or before the
  instant held nothing then: its part is empty, and the answer names no
  position for it.

The read's other instants are one for the whole read. A `valid_at` is an
instant in the world, the same in every lake, and every part is read at
it. The clock a read judges computed fields and listings under is the
instant the read began, one for every part; a cursor carries it, as a
cursor of one lake carries its first page's clock, so every page judges
under the same clock.

A retention hold that stands at a lake's head governs every position of
that lake (006), in its part as in any read of it.

### The parts are put together by the store's one order and its one arithmetic

A row names its lake beside its key: keys are a lake's own, and the same
key in two lakes is two rows.

- **Rows** follow the read's sort keys under the one order every answer
  has (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`
  § `Every answer of more than one thing has one order`), ties broken by
  the key, then by the lake's name. Each lake answers its part in that
  order a page at a time, and the store merges the parts; a cursor
  carries, per lake, its position and the last row taken from it. A
  sort key `latest` is refused across lakes, naming the key: it orders
  by position, and two lakes' positions are not compared.
- **`after`** is refused across lakes: it names one sort position and
  key, and no lake or position. A read across lakes continues by its
  cursor.
- **`total`** is the sum of each lake's `total`, each counted as that
  lake counts it, and the count of retired records the sum of each
  lake's.
- **A view's parameters** are bound once and read in every lake; they
  are part of the view's shape, so every lake declares them alike.
- **Aggregates** are the store's, decimal-exact
  (025 § `Aggregates are the store's, except a count alone`), computed
  over the rows of every part; a count alone is each lake's count,
  added.
- **A view** is read across lakes as any type is: each lake answers its
  view's records, kept or computed
  (`docs/decisions/049-a-view-the-fold-keeps.md`), as rows of that lake.
  An aggregate across lakes over a kept view's rows reads each lake's
  groups, not the records under them, where that lake answers the view
  from what it keeps (049), and computes them otherwise: a sum per
  account over thirty companies that each keep the view by account
  reads thirty companies' groups.

### The mirror stays the way to keep another lake's water

What one lake must hold of another, because its own writes, references,
sets, triggers or kept views act on it, still crosses only as a mirror
(042). A read across lakes reads; it writes nothing, references nothing
and keeps nothing.

### Each lake's limits hold its part, and a setting bounds how many lakes one read names

A lake's read limits are its own `Location` settings, and each part is
read under its lake's. The read as a whole is held to the smallest of
them: a `limit` above any named lake's `max_limit`, or an `offset` above
any named lake's `max_offset`, is refused as a read of that lake alone
would refuse it, naming the lake, and a read that names no `limit` takes
the smallest page any named lake would give it.

How many lakes one read may name is bounded, 32 unless the host sets
it: a setting of the server's configuration, and an argument of the
function where a host calls it in process. A read naming more is refused
`too_large`, never cut, as an `offset` past its bound is. The bound is
what caps the read's cost, which grows with the number of lakes: N
times its `offset` and page, and for an aggregate over a type every
record of N lakes.

### It costs a read of every lake named, one shape across them, a surface of its own and a raise of the budgets

- **Time and room.** A read across N lakes is N reads and a merge. Its
  time is the slowest lake's part where the parts are asked side by
  side, and its room a page from each lake, and with an `offset` that
  many rows more. An aggregate over the rows of a type reads every row
  of every lake, as the same aggregate in one lake does today; over the
  rows of a view each lake keeps, it reads their groups.
- **One shape across the lakes.** A type is read across lakes only
  where its shape and its value types are the same in every lake named.
  A field added to one company's type, or a migration rolled out one
  company at a time, refuses every read across lakes of that type that
  names a lake on either side of it, until every lake named has the
  change. A host that keeps its companies on one bundle meets that
  already; one that lets them drift reads them apart.
- **Open lakes.** Every lake named is opened, read-only where the
  process does not hold it, and kept open as a process keeps any lake
  (`docs/decisions/033-a-process-holds-a-lake-once.md`).
- **Surface.** One function beside the host's store facade, as the
  export is, so no method on it or on an engine trait; its generated
  tool and its REST route; and a credential resolved once per lake at
  the doors that resolve one, REST and MCP over HTTP. The MCP door over
  standard input serves one lake as one principal and has no credential
  to resolve in another (033), so it does not offer the read; a host in
  process calls the function with a principal of each lake.
  `Catalog`'s reads of one lake do not change.
- **The suite.** The rows it would need: the parts and the whole answer
  as each lake's own reads answer them, at pinned positions and at an
  instant; the refusal of a lake the caller holds nothing in and of a
  field one lake does not show; a masked value and a held record in one
  part; the merge and its ties across lakes; a cursor across a write to
  one lake; a sum and a count over a kept view's rows across lakes
  against the same over every lake's records; the refusals of a value
  type declared otherwise in one lake, of a limit past one lake's
  bound and of too many lakes; an instant before one lake's first
  event.
- **The budgets.** At this record's proposal the core crates stand at
  82 714 product lines of 83 000 and 154 149 specification lines of
  154 500 (`docs/budgets.md`), 286 and 351 lines of room. Shipped work
  of like kind measured, in Rust lines added net across the workspace:
  `total` and `offset` on `find` (lakelands#411) about 230 product and
  280 specification, a set over a kept view (lakelands#1221) about 1 000
  and 1 400. A read across lakes, with its merge, its cursor of
  positions, its comparison of shapes, its limits and its rows, is
  nearer the second. Building it takes room the round does not have:
  cuts first, then a raise of both bases by the rule of
  `docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`, with
  its measured row, in the pull request that builds it.

### What it changes

- **042.** "Nothing reads across lakes" (§ `Types cross from one lake
  into another only as a mirror`) and "No query across lakes" under
  "What it is not" become: no lake reads another, and no answer of a
  lake depends on another's state; a read across lakes is answered
  beside the lakes, by this record. "No reference into another lake"
  stands.
- **The pages that say the same**: the README's paragraph on lakes,
  `docs/hosting.md` ("No query across lakes"), `docs/seams.md` on
  mirrors, and `LakeId`'s doc ("Nothing is ever read or written across
  two lakes"), which becomes: nothing is written across two lakes, and a
  read across lakes is answered beside them.
- **009.** An answer across lakes carries one position per lake.
- **The README's "Choose something else when".** The line "you need
  queries across tenants" goes. What stays true is said where the reads
  are described: a read across lakes is a `find`, each lake answers its
  part under its own grants, and there is no join across lakes.

## Why

A lake is one world because every answer in it can then be checked
against its own log. A read across lakes keeps that, if each part is a
read of one lake at one of its positions: anyone can read each lake
again at the position the answer names and get its part back. What 042
guards against is an answer of one lake that rests on another lake's
state, and a lake that is no longer whole once it is exported. A read
answered beside the lakes does neither.

Two other shapes were weighed and not taken.

- **A lake that mirrors the others, with a kept view over the copies.**
  Reads of that view would then answer across companies, at one
  position, as fast as a read of one lake. But it costs a second copy of
  every type that crosses, an answer that lags each source by its
  mirror, and grants kept twice: what a reader may see in the
  consolidation is the consolidation's grants, not each company's, so
  every change of a company's grants must be made again there or the two
  disagree. A mirror maps each source type to a type of its own (042),
  so thirty companies are thirty types and a view that merges thirty
  sources, each with measures of its own, declared again for every
  company added; merging them into one type would break the rule that a
  type has one source and a field one home, and keys that collide
  between companies would need a re-key on the way in. The mirror stays
  for what a lake must hold; it is too heavy as the way to read.
- **Leaving it to the host**, which can call each lake's `find` and
  merge. A host can do that today and gets no common order, no exact
  aggregate, no cursor across pages and no single refusal when one lake
  holds nothing for the caller. Each of those is a rule the store
  already keeps for one lake; the store keeps it across lakes too.

Resolving the caller once per lake keeps the grant model as it is. A
principal with grants in several lakes would need its own rules for
which lake's grant governs a field, and every lake's grants would mean
something different when that principal reads it. One principal per
lake, each part read as that lake reads it, needs no rule at all.

Refusing a read when one lake cannot answer its part, rather than
leaving that lake out, is what makes a consolidated number safe to
report: it is either over every lake named, or it is not answered.

## What it is not

- **Not a join.** No reference crosses lakes, so no walk, path or
  pattern does; a pattern across lakes would be the same pattern in
  each, which is a later step if a host asks for it.
- **Not search across lakes**, in 0.19.0 or in this record. A lexical
  score reads each lake's own weights for the type
  (025 § `A type weighs its searches by a setting`), and a fused rank is
  a rank among one lake's hits, so the scores of two lakes are not
  comparable today; a search across lakes waits for that to be
  settled.
- **Not a write across lakes**, and no transaction across lakes.
- **Not a cut across lakes.** Each lake's part stands at its own
  position; a pin to one instant is as exact as the writers' clocks.
- **Not a kept view across lakes.** Nothing outside a lake is kept.
- **Not a grant across lakes**, and no principal of several lakes.
- **Not a change feed across lakes.** The feed stays one lake's log
  (`docs/decisions/004-the-change-feed-is-the-log.md`).
- **Nothing of it is built.** It is paused (above); a read across lakes
  is built only after a record takes it up again.

## How it came to be

- Accepted on lakelands#1376 (2026-10-24), to be built in a later round.
- Paused on lakelands#1400 (2026-10-25): no host case for one grouped
  read over several lakes was found, so nothing builds it by accident.
