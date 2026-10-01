# 018 — One pattern query, and never a second planner

## What is decided

Walking relations is one query: a start (a type with a key, a filter or
a search), a list of steps (relation, direction, target type, filter,
depth), and a projection (rows for agents, a graph for viewers, or
aggregates). `neighbors`, `traverse`, `path` and `subgraph` are named
constructions of it, not doors of their own. Every answer carries its
position and can be pinned with `as_of` and `valid_at`.

The planner knows the ontology. A step over a relation its type does not
declare is refused, not answered empty, and grants apply per step,
because every step has a type.

Any later query language a caller writes in (a read-only openCypher or
GQL subset, for example) is translated into this query. There is never a
second planner and never a write path through a query language.

## Why

A graph read that returns nothing because of a misspelt relation looks
exactly like a graph read that found nothing. Refusing undeclared steps
turns a silent wrong answer into a clear one.

Keeping one query with named special cases, rather than one door per
graph operation, keeps the tool list short (014) and gives every graph
read the same time axes, paging and rights. Translating any later
language into it, rather than planning that language separately, means
masking, rights and time behave identically whichever syntax a caller
used.

## What it is not

- It is not a general graph database. Depth is bounded by the lake's
  limits, and a step walks only declared relations.
- Aggregation across pattern steps is not in `0.1.0`; aggregation of the
  matches is.
- `search` as a start does not make the pattern query a search door;
  `search` stays its own ranked door (014).

## Extended: too deep is refused, not shortened

A step that asks for more hops than the lake's `max_depth`, or whose
`min` is above its `max`, is refused as invalid. It is never quietly
shortened to the limit: an answer that walked less than it was asked to
would look complete when it is not, and the caller could not tell. A
`subgraph` pattern's steps are one walk over any of their relations,
not a chain of steps.

## Extended: views are computed on read

A view type's records are computed when they are read: the source's
records at the read's position, filtered by the view's `where`, grouped by
its `group_by` fields — whose values, in declared order, form the view
record's key by the same rule as any record's key (`docs/log-format.md`
§2.3), so a `group_by` field is of a kind a key field may be — and
measured, each measure landing in the kind the view
declares for it. A group with no member has no record. Keeping a view's
records materialised is an engine's optimisation, never a rule: it may
make a read faster and may never change its answer, and the conformance
checks on views bind it. A view declares the fields it computes, one for each
`group_by` field and each measure, and no other: a read answers declared
fields only, so a computed value with no field would be dropped without
a word, and a field nothing computes would always be empty; `declare`
refuses both, naming the field. Every read of a view reads those computed
records, the pattern query included: a pattern that starts on a view,
or a walk that reaches one over a derived relation, finds the records
`find` would, and never asks an engine for records it does not keep. A
view is not searched: the indexes a search reads hold stored records
only, so a search of computed records could only answer nothing. A view
field that is searchable, carries an embedding or is a vector is
refused when the view is declared, and a search on a view is refused,
naming it; neither is answered empty.

## Extended: a view may take parameters bound at read

A view may declare parameters, each a name and a scalar kind (text,
number, decimal, boolean or date_time), and read them in its `where`
and its measures as `param('name')`. The read binds them: `find` and
`get` on the view, and a pattern that starts on it, carry `params`, and
every declared parameter must be bound. One view then answers the window
around any date its reader names, which is the reader's question and not
the view's: a thirteen-week cash flow anchored on the date a person asks
about is one view, not one per date.

There is no default. A parameter a read forgets is refused, naming it,
because a default would answer a window around a date nobody asked for,
and that answer would look complete. A reader who means today passes
today. A date given without a time is midnight UTC, so
`date <= param('anchor')` leaves out the rest of that day; a reader who
means its end passes the next day with `<`, or the instant itself.

`param()` is read only in a view's `where` and measures, where the view
declares what it reads; anywhere else it is refused. A parameter is a
value, never the clock: `today()` and `now()` stay the read's time, and
`as_of` and `valid_at` combine with the parameters as they combine with
each other. A view's parameters are part of its shape, so changing them
is a change of shape like any other. A view that declares none keeps the
schema hash it had. A cursor carries the parameters its first page was
read with, and continuing it with others is refused, since the pages
would belong to two answers. Views are computed on read, so nothing is
kept per binding.

## Extended: a whole-graph measure is a projection

Degree centrality and connected components are two more projections of
the one query, `degree` and `components`, not doors of their own. The
start picks the records measured; each step names a relation and a
direction. The answer is rows with fixed columns (`type`, `key`,
`degree`; or `component`, `size`, `type`, `key`), paged like any rows,
so a caller learns no new result shape. A measure refuses a step
`depth` or `where`, `to` and `order_by`, and `components` refuses a
step that does not walk `both`: each would be ignored otherwise, and an
answer that ignored what it was asked would look like more than it is.
Components hold every record they reach, so the lake bounds them by its
`max_graph_nodes`; past it they are refused `too_large`, naming the
ceiling, for the reason a step too deep is refused: a partition of part
of the graph would look like the whole of it.
