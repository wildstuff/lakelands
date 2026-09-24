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
