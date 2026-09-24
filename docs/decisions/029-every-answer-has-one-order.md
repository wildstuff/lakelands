# 029 — Every answer has one order

## What is decided

Whenever Lakelands answers with more than one thing, the order is part of
the answer and the same on every engine:

- **Sorted records** follow their sort keys, a record without a value for
  a key after every record with one in either direction, and the record
  key ascending to break ties. Values are compared by one function
  (`lakelands_core::expr::order`): ranked by kind first — numbers (and
  text that reads as one), then instants, then text, then booleans, then
  JSON values — and within a kind as the evaluator compares them.
- **Walks** order what they reach by depth, then type, then key, and links
  by their ends (`docs/decisions/026-the-store-walks-the-graph.md`).
- **Ranked search** breaks equal scores by key. Hybrid search fuses its
  lexical and vector rankings by reciprocal rank (each ranking adds
  `1 / (60 + rank)` per record, rank from 0), then breaks ties by key.
- **Groups** come in the order of their key's canonical bytes.
- **Aggregates are the store's.** Every group and measure — of `find`,
  of a pattern's rows, of a view — is computed by the store over the
  records it reads, decimal-exact, never by an engine. An aggregate an
  engine computed would be a second implementation of the same answer,
  free to round or order differently; with one, every engine answers
  identically by construction.

## Why

A page set, a cursor and "the first ten" only mean something if every
engine and every run puts the same records in the same places. An order
left to an engine — a hash map's iteration, an index's physical layout —
looks deterministic until the day it is not, and then a cursor skips a
record or an agent sees a different "top result" for the same question.
The evaluator's own comparison is not a total order across kinds, so a
sort needs a ranking of kinds on top of it; writing that ranking once
keeps every engine's pages identical.

## What it is not

- It is not a promise that relevance scores are identical across engines;
  equal scores are ordered the same way.
- It does not forbid an engine from using its own indexes, as long as the
  answer comes out in this order.
