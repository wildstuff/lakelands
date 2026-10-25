# 080 — A pattern may branch and join

## What is decided

**Accepted** on lakelands#1404 (2026-10-25, by the operator), for
0.20.0. It amends `docs/decisions/018-one-pattern-query.md` and comes in
force with the issue that builds it; until then a pattern is a chain or
a neighbourhood, as 018 says.

### A pattern is a tree of steps, each walked from a name bound earlier

A step may name, with `from`, the name it walks from: the start's, or
one a step before it in the list binds. Two or more steps from one name
branch: `(a)-[:x]->(b), (a)-[:y]->(c)` is the start `a` and two steps
`from: a`. A step that names no `from` walks from the name the step
before it binds, the start's for the first, so a pattern that names none
is the chain it is today and answers as it does. A `from` that names a
name no earlier step binds, a later step's included, is refused as
invalid, naming the step and the name: steps are listed in an order a
walk can follow, and a pattern never names a record before it is bound.

A row is one match of the whole tree: a record for every name. Where a
name has several branches, a start's rows are every combination of its
branches' matches, as a row of a chain is every combination of its
steps'.

### A join reaches a record an earlier name bound, and closes a cycle

A step that names `join` walks from its `from` and holds only where it
reaches the record the joined name already bound:
`(a)-[:entered_by]->(p), (a)-[:approved_by]->(p)` is a second step
`join: p`. It binds no name of its own, so its `as` is refused, and its
`type` and `where` are refused too: the record it reaches is judged
where its name was bound. A join to a name no earlier step binds is
refused as `from` is. A join whose relation, over its whole range, can
reach no type the joined name binds is refused before a record is read,
naming the step, the name and the types the step reaches, as 018
refuses a `type` its step does not reach: a join that could never hold
would answer as if the lake held no such cycle. The hops between its
`from` and the joined record bind nothing, as a chain's inner hops do.

### An optional step binds null where it has no match

A step with `optional` keeps its row where it has no match: the row
holds `<name>.key`, `<name>._type` and `<name>._depth` as null and no
field of the name, so a host tells "nothing there" from "a record with
no value" by its key. A step that walks from a name an optional step
binds, or joins one, is optional too, and is refused unless it says so:
an optional step and the steps that walk from it are one branch that
matches whole or binds null whole, so a step past an optional one never
drops the row the optional step kept. A join inside an optional branch
to a name outside it is a condition of the branch.

A null sorts as an absent value does, a group key over it is null, and
a measure reads it as absent (025).

### An absent step keeps its row only where it has no match

A step with `absent` keeps its row only where it has no match from its
`from` record, its `type`, `where` and depth range judged as a step's
are: `where not exists`. With `join` it asks that no walk over its
relation reach the joined record, so the bills their own enterer did not
approve are the rows of `(a)-[:entered_by]->(p)` with an absent step
`from: a` over `approved_by` that joins `p`. It binds no name, so its
`as` is refused, and so is a step that names it as its `from`. A record
its reader is not shown, or one its reader's `only` does not admit, is
no match, as it is in every walk (006): an absent step answers alike
whether a hidden record is there or not.

### Each keeps 018's rules

- **One order.** Rows follow `order_by`, or come in the starts' order
  and each start's in the walk's (026): matches compared name by name in
  the order the pattern binds them, each by depth, type and key, and a
  name bound to null after every record. The order does not depend on
  the order an engine returns links in.
- **One cursor.** A cursor continues one pattern: its digest covers
  every step's `from`, `join`, `optional` and `absent` (018), so a page
  of one tree never continues another.
- **Rights per step.** Every step has a type, a join's and an absent
  step's included: each step's relation is probed around the type it
  walks from, and its `where` probes what it reads, before a record is
  read (006). An optional or absent step that reads a field its reader
  may not read is refused as any step is, never answered as if it had
  not matched.
- **Refusal before a record is read.** Each step's `from` and `join`,
  the members a projection takes, and an optional branch's closure are
  judged before any step's relation, in the order of the steps, and the
  first fault is the answer, so a refusal never depends on what the
  lake holds.
- **Ceilings refused, never cut.** Each step's depth is held to
  `max_depth` as today. Branches multiply rows, never what a walk holds:
  each step's records are held once and rows made from them in order.
  Rows read whole, under an order over a step's record or for an
  aggregate, are at most `max_graph_nodes`, refused `too_large` past it,
  as a chain's are (026).
- **The same answer on every engine.** The tree is walked in the
  store's core over the two questions an engine answers, the links of a
  record and whether it is visible (026), and a derived relation's
  matching records (044). No engine method is added, and the rows that
  hold it run on every engine (031).

### A tree is for rows and an aggregate

`from` naming any name but the one before it, `join`, `optional` and
`absent` are taken by a `rows` and an `aggregate` projection. A graph, a
path, a measure and `reach` refuse each as invalid, naming the member,
before a step is judged: each answers by a walk's own shape, and an
answer that ignored a member would look like more than it is.

## Why

A question a person asks of a graph is rarely a chain. Who approved a
bill they also entered is a cycle; a customer with an open bill and no
contact person is an absent step; a project with its lead, if it has
one, is an optional one. Today each takes several queries and a merge
the host writes, with no common order, no cursor over the whole and the
rights judged per query, not per answer. A query language a caller
writes in (018) would translate these first; without them it would be
the pattern spelled differently.

Keeping the tree in the one pattern query, walked in the core, keeps
every rule 018 already holds, rights, time axes, paging and ceilings,
and costs the engine seam nothing.

## What it is not

- Not a graph of a tree. A graph's nodes and pages are a walk's; which
  records a tree's graph holds is left to a record of its own, if a host
  asks for one.
- Not a join across types by value. A join reaches a record over a
  relation; matching records by equal values is a derived relation
  (044).
- Not an optional or absent start. The start is bound in every row.
- Not a second planner. Steps are walked in the order they are listed;
  the store does not reorder them.
