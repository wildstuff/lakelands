# 082 — A query language writes only through the typed write doors

## What is decided

**Accepted** on lakelands#1404 (2026-10-25, by the operator), built in
0.21.0 on the parser of the read-only openCypher subset. It amends
`docs/decisions/018-one-pattern-query.md`, whose "never a write path
through a query language" becomes "a query language writes only through
the typed write doors", and comes in force with the issue that builds
it; until then 018 stands as written.

### Every statement is one call of a typed write door

A write statement translates into exactly one call of a door that
exists: the plan and enact of a change to the records a condition
selects
(`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`),
or one `upsert`, `link`, `unlink` or `retire`. So field rights (006,
012), provenance (010), the two write doors by origin (016), proposals,
positions and the log are those of every other write, and nothing is
written that the doors would not write. The translation produces the
door's call and nothing else; it lives in the subset's own crate, as the
read translation does, and never judges a right, a ceiling or a
position. A statement that would need two calls is refused, naming its
second write clause: two calls are two appends, and a statement that
half landed would look as if it had not run.

Values come as literals or bound parameters, never as text interpolated
into the statement.

### `MATCH … SET` and `MATCH … DELETE` change selected records

`MATCH … SET` sets the values it names on the records one name of the
`MATCH` binds, and `MATCH … DELETE` retires them, or unlinks the links
a relationship name binds; nothing is erased. Each is 081: the
statement sent alone answers the plan, and sent with the id of the plan
its caller was shown it enacts it, refused `plan_stale` where the plan
moved. It is never enacted unplanned. `REMOVE` of a property sets it to
null.

The `MATCH` is translated into 081's condition on the written name's
type: its `WHERE` in the expression grammar (017), and the rest of the
pattern only where that grammar says it of that type. A `MATCH` whose
selection needs a second name the condition cannot say is refused,
naming the construct. 081's read rule holds for every `MATCH` that
selects what a statement writes: the condition reads only what its
caller may read, refused before a record is read.

### `MERGE` keys on what keys a record, and `CREATE` expects none

- **`MERGE`** is an `upsert` only on a type's declared key, every key
  field given and no other field in its pattern, and a `link` only on a
  link's pair (`from`, `to`), which keys a link record (035). A `MERGE`
  on any other field is refused, naming it: two callers merging on a
  field that keys nothing would each create a record. `ON CREATE` and
  `ON MATCH` are refused, naming them: a door writes one patch, whether
  the record is there or not.
- **`CREATE`** of a node is an `upsert` that expects no record under its
  key, and of a relationship a `link` that expects no link of its pair:
  where one is there, it is refused as the door refuses a version that
  moved.

### Refused, naming the construct

Each is refused before a record is read, with the construct's name, as
the read subset refuses what lies outside it:

- a write to a computed or derived field, also where the door would
  refuse it by its authority, so the caller learns it from the
  statement and not from a record;
- a schema change: a label set or removed, an index or a constraint;
- `DETACH DELETE`: a retirement that unlinks what it names is two doors,
  and a reference left naming no live record is told by the plan, never
  removed for the caller (032);
- a write whose value depends on a `WITH` aggregate, or on a record's
  own fields: 081 sets values;
- `CALL`, and `FOREACH`;
- a `RETURN` beside a write: the statement answers its door's outcome.

### The doors that serve it

Write statements are served at REST, the CLI and the client. Walk 6
(`docs/walks/walk-6.md`) decided whether the subset is an MCP tool: it
is, as the `cypher` read tool beside `query`, never in its place, and
that tool takes no write statement; it refuses one naming its clause.
An agent at the MCP door changes many records through 081's typed
tools, whose plan is structured so a host can word it for the person
who confirms it.

## Why

A developer or a reporting tool that reads a lake in openCypher will
want to correct what it reads in the same words. A second write path,
planned apart from the doors, would need every right, provenance rule,
origin rule and refusal again, and would drift from them. Translating
each statement into a door that exists keeps one place where a write is
judged, as translating each read into the one pattern query keeps one
planner (018).

`MERGE` on what keys nothing is the classic race of a graph language:
two callers each find nothing and each create. On a declared key, or on
a link's pair, the lake already holds one record at most, so `MERGE`
says what the door does.

## What it is not

- Not a transaction language. One statement is one door call.
- Not a way around a plan. A change to many records is planned and
  confirmed whatever language asks for it.
- Not a write tool for agents in 0.21.0.
- Not built in 0.20.0.
