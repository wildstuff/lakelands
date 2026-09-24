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
