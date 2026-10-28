# 091 — A field-carried relation over an array of keys yields one edge per key

**Proposed** on lakelands#1551, for 0.21.0. The operator reviews it on the issue; merging it does not accept it.

## What is decided

A relation carried by a `via` field links a record to the target record
whose key is the field's text. A field that holds a JSON array of keys
yields no edge at all: the one rule every engine applies,
`lakelands_core::engines::links_of`, takes the field as a string or not
at all. A host that keeps a list of other records' keys in one field (a
type that implements several others, a node derived from several
sources, a learning that cites several earlier ones) walks none of them.

This record needs no log form, no format step and no engine work. The
edges of a `via` relation are not in the log: every engine derives them
from the head states and the states at a position, with one function
(`docs/decisions/031-engines-share-one-rulebook.md`). It extends
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

### Every reader sees the same edges, and the build changes two places

1. `lakelands_core::engines::links_of`: the field's value is read as one
   key or as an array's distinct string elements, by one function beside
   it that answers a value's keys. The module doc's "Links" rule says so.
2. `lakelands-store`'s purge report (`purge.rs`, the `inside` test): a
   record counts toward a relation where its `via` field names a purged
   record, alone or as any element, and a record is counted once,
   however many of its keys the purge takes (039 as extended).

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

### A lake upgraded from format 20 is back-filled by `rebuild`

0.21.0 derives array edges from every state, whatever the format of the
event that wrote it. An upgrade from format 20 rewrites nothing and
rebuilds nothing: the 20 → 21 step of `docs/log-format.md` §6 stays as
it stands, since the fold does not change, and neither engine's
projection layout moves.

So on a SQLite or PostgreSQL lake upgraded in place, a record whose
array was written before the upgrade and not since has no edge in the
head's `edges` table: a walk at the head answers none of its array
edges, while a read at an earlier position, which derives them from the
state, and the purge report, which reads the field, do.
`lakelands rebuild` (or a host's open with `OpenMode::Rebuild`)
back-fills them, and writing the record again fills its own. A memory lake does not
outlive its process, and an import replays, so neither has this window.
`docs/hosting.md`'s notes on upgrading to 0.21.0 say it in one
sentence. No host lake is known to hold such an array.

## Why

The host keeps these lists in one field because the field is the record:
a learning is written with what it cites, in one write, and a cited
learning may sit where this lake never holds it. One edge per element on
the path the field already carries is what the host needs, and nothing
else moves: no migration, no second carrier, no write road kept in step.

**It keeps 002.** `docs/decisions/002-determinism-is-the-boundary.md`
asks that a replay reproduce what the writes gave. The edges are no
event and no fold state; they are the rulebook's reading of a state the
log determines. A write under 0.21.0 and a replay under 0.21.0 fold the
same state and read the same edges from it, so every lake 0.21.0 wrote,
or imported, answers the same edges live, after a rebuild and after a
reopen (Check 1). Nothing recorded depends on them (above), so a rebuild
changes the edges index and no event or record.

**Array edges are derived for states of format 20 and before on a
replay.** A field holds a JSON array the same way in format 20 and 21,
and the state carries no format: a gate by format would have two records
with the same field link differently forever, an answer no reader could
explain from what it shows. It would also have every engine's derivation
and every read at a position ask the format of each record's last write,
code for a difference nobody reads. And it would leave `rebuild` unable
to back-fill. What a projection held before the upgrade is the earlier
release's reading, which `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`
already leaves to the engine and a rebuild
(`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).

## Rejected

- **Link records instead.** A pinned profile refuses to change a
  relation's carrier (`carrier_changed`), a link write refuses a key with
  no record behind it, and the field and the links would hold each edge
  twice, every write road keeping them in step (lakelands#1551).
- **Rebuilding at the upgrade.** Raising the SQLite and PostgreSQL
  layouts would have the first writer's open of 0.21.0 rebuild every
  lake (about 8 s per 100 000 records on SQLite, 47 s on PostgreSQL) to
  back-fill a field no known lake holds. The host that needs it runs
  `rebuild`.
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
  +10; `lakelands-api`: the doc of `RelationDecl::via`, about +2 to +5.
- **Specification (core crates), est.: +320 to +490**, the five checks
  and the unit rows.
- **Outside the core:** the sentence in `docs/hosting.md`, and the
  regenerated `docs/conformance.md`.
- **Budget rows.** Core product stands at 94 202 of its 94 500 ceiling,
  and 090's build (+350 to +500, decision record 090 on lakelands#1550)
  takes it to at most 94 702 under the 95 000 its own row sets; this
  build's top leaves it at 94 752. It needs no product row. Core
  specification stands at 173 692, and 090's build (+450 to +650) takes
  it to at most 174 342 under the 174 500 its row sets; this build's top
  takes it to 174 832. It needs a specification row of +500: 174 500 →
  175 000, where 090's build lands first, as 0.21.0's order expects;
  where this build lands first, the row is 174 000 → 174 500 and 090's
  moves up by the same. The checks pin the integrity of the answers
  every engine gives, by the rule of
  `docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`.

## How it came to be

- Proposed on lakelands#1551, for 0.21.0.
