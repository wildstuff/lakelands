# 026 — The store walks the graph; engines answer edges

## What is decided

Graph walks — one hop, a chain of steps, a subgraph, a shortest path —
are implemented once, in the store's core, over two questions an engine
answers: the links of a record, and whether a record is visible. Engines
never implement a walk. Every walk orders its own answer (records by
depth, type and key; links by their ends), so the answer does not depend
on the order an engine returns links in.

## Why

A walk implemented by each engine is a walk that can differ between
engines, and the difference would show only on the graph that exposes
it. Two primitives are enough for every walk the pattern query needs,
and keeping the walks in one place keeps the engine seam narrow and the
answers identical.

## What it is not

- It is not a limit on performance work later. A faster walk is a change
  to the store's implementation, checked against the same answers.
- It does not follow a link to a record that no longer exists or is
  retired; that end is not visible.

## Extended: a graph includes its start

A graph projection always includes the start node, the root a viewer
draws from, whatever the `min` of its steps. A rows projection holds a
start only when a step reaches it. Either way the answer keeps the one
order: depth, then type, then key.

## Extended: the whole-graph measures are walks

Degree centrality and connected components are computed once, in the
store's core, over the same two questions every walk asks, so an engine
answers them without knowing they exist. They count what a walk would
follow: link records and via references live at the read's position, a
far end that exists and is not retired and is of a type the caller may
see, a restricted link only for a caller with the grant. A degree reads
one record's links at a time and the start's records a page at a time,
so measuring a whole type holds a page and the best rows asked for, not
the type. Components hold every record they reach, up to the lake's
`max_graph_nodes`, and are refused `too_large` past it, never cut.

**A measure's page.** A read keeps nothing between calls, and a page in
the middle of a measure cannot be made from less than the measure: a
component's number and size are known only when the whole partition is.
So a measure is computed whole for every page, and a walk through small
pages computes it once per page (lakelands#474). The way to one
computation is one page: a measure's `limit` may go up to the lake's
`max_graph_nodes`, the bound the lake already puts on a whole-graph
answer, where every other read stops at `max_limit`. A components answer
always fits that page. An absent `limit` stays the lake's default, so an
agent that names none is not handed the whole answer. Small pages are
still answered, at their price; the `query` tool says so.
