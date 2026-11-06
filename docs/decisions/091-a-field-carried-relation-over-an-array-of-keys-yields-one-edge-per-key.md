# 091 — A field-carried relation over an array of keys yields one edge per key

Decided on lakelands#1551 by the operator (2026-10-29), for 0.21.0, with the projection layouts raised.

## What is decided

A relation carried by a `via` field links a record to the target record
whose key is the field's text. A field that holds a JSON array of keys
yields no edge at all: the one rule every engine applies,
`lakelands_core::engines::links_of`, takes the field as a string or not
at all. A host that keeps a list of other records' keys in one field (a
type that implements several others, a node derived from several
sources, a learning that cites several earlier ones) walks none of them.

This record needs no log form and no format step. The edges of a `via`
relation are not in the log: every engine derives them from the head
states and the states at a position, with one function
(`docs/decisions/031-engines-share-one-rulebook.md`). The SQLite and
PostgreSQL engines raise their projection layouts, so that a lake
0.20.0 left is rebuilt before it answers. It extends
`docs/decisions/039-a-source-is-retracted-as-one.md`, whose purge report
counts a reference, and comes into force with the build, which rewrites
that section where it stands.

### An array of keys yields one edge per distinct key

Where the `via` field of a record holds a JSON array, each element that
is a string is a key, and the record has one edge of the relation to the
target record of each distinct key. Only a `json` field can hold an
array (a `text` or `reference` value is a string), and a `via` over a
`json` field is declared as it is today; nothing new is declared.

- **A string element** is a key as a single-string field's value is:
  taken as written, the empty string included.
- **An element that is not a string** (a number, a boolean, `null`, an
  array or an object) yields no edge. It is not read as a key (`7` is
  not the key `"7"`), a nested array is not flattened, and an object is
  not searched for a key. It stays in the field and nothing is refused:
  the field is `json`, and a single value that is no string yields no
  edge today.
- **A key the array holds twice** yields one edge. An edge is its two
  ends and its relation, as a link record is one per pair
  (`docs/decisions/035-a-link-is-a-record.md`); `links_of` answers each
  relation's distinct keys in key order, so no engine stores or answers
  it twice.
- **An empty array** yields no edge.
- **A single string** yields its one edge, exactly as today, and a field
  that is absent or `null` none.

### A rewrite moves the edges, and a key with no record is kept

The edges are derived from the record's state, so a write that drops a
key from the array drops that edge, and one that adds a key adds it, at
the write's position. An earlier position answers the edges its state
held. Moving a key within the array changes no answer: no edge carries
its element's place.

A key that names no record stays in the field and is not refused: a
`json` field is no `reference`, which alone a write judges
(`docs/decisions/032-a-reference-names-a-live-record.md`), and the cited
record may live where this lake never holds it. Its edge leads nowhere a
reader is shown: a walk answers no end that is not a live record its
reader may see (`docs/decisions/026-the-store-walks-the-graph.md`), so
it answers neither the edge nor a node. When a record of that key is
written later, the edge reaches it from that position on, as a
single-key field's does today.

A relation's `cardinality` is not judged for a `via` relation, as today:
a host declares such a relation `one_to_many` or `many_to_many`, and
`describe` tells it as declared.

### Every reader sees the same edges, and the build changes these places

1. `lakelands_core::engines::links_of`: the field's value is read as one
   key or as an array's distinct string elements, by one function beside
   it that answers a value's keys. The module doc's "Links" rule says so.
2. `lakelands-store`'s purge report (`purge.rs`, the `inside` test): a
   record counts toward a relation where its `via` field names a purged
   record, alone or as any element, and a record is counted once,
   however many of its keys the purge takes (039 as extended).
3. The SQLite and PostgreSQL projection layouts (`LAYOUT`, 7 → 8 and
   9 → 10), below.
4. The doc of `RelationDecl::via` in `lakelands-api`, which says the
   field holds a key or an array of keys.

Everything else reads through `links_of` and does not change:
- the edges each engine keeps for the head: the SQLite and PostgreSQL
  `edges` tables and the memory engine's `in_edges`, filled from
  `links_of` at every head state the fold hands them and cleared by
  source when the record is written again, so a rebuild fills them the
  same way;
- each engine's `edges` at an earlier position, and `in_links`, which
  filters `links_of` to the record walked into;
- the graph walks, measures and pattern rows of 026 and
  `docs/decisions/018-one-pattern-query.md`, and the openCypher subset
  over them, which ask the engines' `edges`;
- the rights of `docs/decisions/006-grants-not-roles.md` over a relation
  carried by a field, which judge the field and not its value: to a
  reader who may not know of the field the relation is not shown, one
  who sees it masked is refused the walk, and where a hiding lowers the
  field on a record, none of that record's edges exist for its reader,
  every key of its array with them.

The cross-record reads of a write (`writes/across.rs`) do not change. A
path steps across a `reference` field alone, which holds one key, and
reads into a `json` field as a value
(`docs/decisions/055-a-path-steps-through-a-reference.md`). A step that
reached several records would make a path many-valued, which the
expression grammar does not have
(`docs/decisions/017-one-expression-grammar.md`). So a condition, a
derived field, a set or a trigger reads no edge of a `json` field, and
nothing the fold records depends on these edges.

### What the doors show, and no WIT change

An edge keeps its shape: `from`, `relation`, `to` and no `fields`. The
graph answer's `edges`, a rows projection's bindings (one row per record
reached, so an array of three reaching three records binds three), a
degree (each distinct key once) and the components carry what the walk
reached, through MCP, REST, the CLI and the client as they carry them
now. The order and the pages are 026's and
`docs/decisions/029-every-answer-has-one-order.md`'s: records by depth,
type and key, links by their ends, and no element's place enters either.
`describe` tells the relation and its `via` field as now. There is no
WIT change, no catalog change and no new refusal.

### A lake 0.20.0 left is rebuilt by its first writer's open

0.21.0 derives array edges from every state, whatever the format of the
event that wrote it. The log is rewritten for none of it, and the 20 →
21 step of `docs/log-format.md` §6 changes no fold. But the edges table
of a SQLite or PostgreSQL projection that 0.20.0 made holds no edge of
an array, and each engine answers the head from that table and an
earlier position through `links_of`. Read as it stands, such a lake
would answer a walk at the head without an array's edges, and a read as
of the position before, a rebuild and the purge report with them: a
projection that differs from its replay, which
`docs/decisions/001-the-log-is-the-truth.md` calls a bug.

So both engines raise their projection layout: SQLite 7 → 8,
PostgreSQL 9 → 10, each unchanged since 0.20.0. That is the layout's own
rule: it is raised whenever a table changes so that a projection made
before would answer wrongly, not fail. A projection of the layout before
is not read. A reader's open is refused `projection_layout`, and the
first writer's open of 0.21.0 rebuilds it from the log under its lease
and says why, as for any layout step (`docs/hosting.md`, "A projection is
read only by the layout that made it"). On PostgreSQL the same open
first converts the log's columns, as
`docs/decisions/086-an-engine-keeps-an-events-id-once-and-its-hashes-as-bytes.md`
decides. `lakelands build-ahead` with 0.21.0, run while 0.20.0 still
serves the lake, folds the log beside the projection, so that the open
folds only the tail
(`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`). A
memory lake does not outlive its process, and an import replays, so
neither needs it. `docs/hosting.md` names 0.21.0 beside 0.16.0 as a
release whose first writer's open rebuilds the projection, and
`docs/log-format.md` §6's 20 → 21 line says that the projection is
rebuilt by the layout, not by the step.

### A rebuild at the upgrade, against the earlier plan

The plan the operator accepted with lakelands#1551 said the upgrade
rebuilds nothing, and a host runs `rebuild` to back-fill a lake that
holds arrays already. This record departs from that, for 001: until the
host ran that rebuild, a lake upgraded in place would answer its head
otherwise than its own replay, and nothing would tell the host so. The
host that asked for this declares such relations, so its lakes are the
likeliest to hold arrays already. The operator accepted the departure.
The rebuild's pause is what any layout step costs. Measured on a lake of
100 000 records, each with an array of keys, the first writer's open took
19.9 s on SQLite, 59.2 s on PostgreSQL and 68.1 s on PostgreSQL with
pgvector; the second open took 0.008 s on SQLite and about 0.06 s on
PostgreSQL.

Rejected: keep the layouts and name the gap as an exception to 001, with
`lakelands rebuild` as the host's step. It leaves a lake that answers
otherwise than its log until someone remembers that step.

## Why

The host keeps these lists in one field because the field is the record:
a learning is written with what it cites, in one write, and a cited
learning may sit where this lake never holds it. One edge per element on
the path the field already carries is what the host needs, and nothing
else moves: no migration, no second carrier, no write road kept in step.

**It keeps 001 and 002.** `docs/decisions/002-determinism-is-the-boundary.md`
asks that a replay reproduce what the writes gave, and 001 that a
projection be its replay. The edges are no
event and no fold state; they are the rulebook's reading of a state the
log determines. A write under 0.21.0 and a replay under 0.21.0 fold the
same state and read the same edges from it, so every lake 0.21.0 wrote,
or imported, answers the same edges live, after a rebuild and after a
reopen (Check 1). Nothing recorded depends on them (above), so a rebuild
changes the edges index and no event or record. A lake 0.20.0 left has
its projection rebuilt before it answers (the layouts, above, and
Check 6).

**Array edges are derived for states of format 20 and before on a
replay.** A field holds a JSON array the same way in format 20 and 21,
and the state carries no format: a gate by format would have two records
with the same field link differently forever, an answer no reader could
explain from what it shows. It would also have every engine's derivation
and every read at a position ask the format of each record's last write,
code for a difference nobody reads. And it would leave `rebuild` unable
to back-fill. What a projection held before the upgrade is the earlier
release's reading, which `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`
leaves to the engine, and the layout makes the engine rebuild it.

## Rejected

- **Link records instead.** A pinned profile refuses to change a
  relation's carrier (`carrier_changed`), a link write refuses a key with
  no record behind it, and the field and the links would hold each edge
  twice, every write road keeping them in step (lakelands#1551).
- **A format gate.** Deriving no array edge from a state of format 20
  or before would keep an upgraded lake equal to its replay without a
  rebuild. But the gate would hold forever: two records with the same
  field would link differently, and every derivation would need the
  format of each record's last write (Why).
- **A rebuild only where a `json` field carries a relation.** The open
  could look at the ontology and rebuild only then. That is a second
  trigger for a rebuild beside the layout, to save a pause that a build
  ahead already makes short.
- **Refusing more than one key on a to-one relation.** The store judges
  no `via` relation's cardinality today, on either side. Judging it for
  arrays alone would refuse the host's writes on profiles already
  declared, for a rule that belongs to both sides at once.
- **Non-string elements as keys, or flattening.** A key is text. A
  number's canonical form (`1e+21`) would make a key depend on number
  formatting, and a nested list has no meaning the host asked for.
- **A `reference` that holds a list.** It would need a new kind, 032's
  judgement per element, and a many-valued path step (017).

## What it is not

- **Not a judged reference.** No element is looked up at the write.
- **Not a rewritten key.** A rekey rewrites `reference` fields only, and
  a key in a `json` field, alone or in an array, stays as written, as
  today.
- **Not a path across.** A condition reads no edge of an array (055).
- **Not ordered.** An element's place in the array is no part of an
  edge or of any answer.

## Checks

On every engine. A type `learning` with a `json` field `cites` carrying
the relation `cites` to `learning`, `many_to_many`.

1. `walks::ARRAY_FIELD_EDGE_PER_KEY`: the case of lakelands#1551.
   Learnings `a`, `b` and `c` exist. `x` cites
   `["a", "b", "b", 7, null, true, ["c"], {"key": "c"}, "", "ghost"]`.
   A walk out of `x` answers the edges to `a` and `b`, each once, and
   no other edge or node. A walk in from `a` and from `b` answers the
   edge from `x`, once; from `c`, none. The field reads back as written.
   `y` citing `"a"` answers its one edge as before, and `z` citing `[]`
   and `w` citing `7` none. The same at a read as of after the writes,
   after `rebuild` and after a reopen.
2. `walks::ARRAY_FIELD_REWRITE_MOVES_EDGES`: `x` written again citing
   `["b", "d"]` with `d` existing answers `b` and `d` out, and `a`'s
   walk in no longer answers `x`; a read as of before the rewrite
   answers `a` and `b`. Written again as `["d", "b"]`, every answer is
   the same bytes. A record `ghost` written later is reached from `x`
   at the head and not as of before its write.
3. `walks::ARRAY_FIELD_EVERY_READER`: over an array reaching three
   records, a rows projection binds three rows, a two-step chain and a
   shortest path go through an array edge, a degree counts each distinct
   key once, and the components hold `x` with its cited records; each
   answers the same at the head and at a position.
4. `walks::ARRAY_FIELD_HIDDEN_WITH_ITS_FIELD`: where a hiding lowers
   `cites` on `x` alone, its reader is answered none of `x`'s edges, out
   of `x` or into `a` and `b`, and `y`'s edge into `a`; a reader who sees
   `cites` masked is refused the walk `not_granted`, naming the field;
   a reader shown it is answered every edge.
5. `purge::REFERENCES_IN_AN_ARRAY`: a purge that takes `a` and `b`
   reports `x`, whose array names both, once under `cites`, and does not
   report a record whose array names neither.

6. Per engine, beside the other older-layout rows (SQLite's and
   PostgreSQL's `older_layout.rs`):
   `a_lake_0_20_0_left_holding_an_array_answers_its_edges_after_the_first_writers_open`.
   The test makes a lake with `x` citing `["a", "b"]` and `y` citing `"a"`,
   then takes it back to what 0.20.0 left: the projection one layout
   back, its edges table holding `y`'s edge and none of `x`'s, and, on
   PostgreSQL, its log as 086's row takes it back. A reader's open is
   refused `projection_layout`, naming both layouts. The first writer's
   open rebuilds and says why. A walk at the head out of `x`, and into
   `a` and `b`, then answers the array's edges, the same bytes as a read
   as of the position before the open and as after `rebuild`. The next
   writer's open rebuilds nothing. Planted: with the layout left as it
   was, the walk at the head answers without `x`'s edges, and the test
   is red.

Unit rows beside `src/`: the function that answers a value's keys, at
a string, an array with every kind of element and a repeat, an empty
array and a value of each other kind.

## Cost

Estimated from `links_of` and the purge report's test as they stand.
Seam width does not move, the facade gains no method, and the engines do
not move.

- **Product (core crates), est.: +20 to +50.** `lakelands-core`: the
  keys of a value and `links_of` over them, with the module doc, about
  +15 to +35; `lakelands-store`: the purge report's test, about +5 to
  +10; `lakelands-api`: the doc of `RelationDecl::via`, about +2 to +5;
  the two engines: each `LAYOUT` raised, no line added.
- **Specification (core crates), est.: +500 to +730**: the five checks
  and the unit rows, +320 to +490, and Check 6 on two engines, +180 to
  +240.
- **Outside the core:** the sentences in `docs/hosting.md` and
  `docs/log-format.md` §6, and the regenerated `docs/conformance.md`.
- **Budget rows.** 090's build has landed (lakelands#1554): core
  product stands at 94 665 of 95 000, specification at 174 498 of
  174 500. This build's top takes product to 94 715. **It needs no
  product row.** Its specification top takes 174 498 to 175 228. **It
  needs one specification row of +1 000: 174 500 → 175 500**, which
  holds that top. The checks pin the integrity of the answers every engine
  gives and of a lake across its upgrade, by the rule of
  `docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`.

## How it came to be

- Proposed on lakelands#1551, for 0.21.0.
- Review: a projection that 0.20.0 laid out would answer its head
  otherwise than its replay. The engines raise their layouts so that
  the first writer's open rebuilds it, and an upgrade check pins that.
- Decided by the operator on lakelands#1551 (2026-10-29): accepted, with
  the layouts raised, so a lake 0.20.0 left is rebuilt by its first
  writer's open.
