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
  A position in this order is a row's sort values and its key: a cursor
  carries one, and `find`'s `after` names one in the open, continuing
  strictly after it at the caller's own read position — the head unless
  `as_of` says otherwise — so a cursor is what pins a page set, and
  `after` is what resumes one. A key `latest` orders records by the
  position of the event that made each one's state at the read's
  position; no two records share one, so it never needs the key to
  break a tie.
- **Walks** order what they reach by depth, then type, then key, and links
  by their ends (`docs/decisions/026-the-store-walks-the-graph.md`).
- **Graph measures** order degrees highest first, ties by type and key;
  components largest first, ties by their least type and key, numbered
  from 0 in that order, and a component's records by type and key.
- **Ranked search** breaks equal scores by key. Hybrid search fuses its
  lexical and vector rankings by reciprocal rank (each ranking adds
  `1 / (60 + rank)` per record, rank from 0), then breaks ties by key.
  The two ties are broken by different keys, on purpose
  (lakelands#826). **One ranking's** equal scores come in the order of
  the keys at the head, where the indexes rank: that is the order an
  engine cuts a list at `k` in, and so the order a cursor follows. A
  search as of a position before a re-key answers each record under its
  key there, and still in that order, so every page is whole; sorting a
  page again by the keys answered would answer some of a large tie twice
  and some never (lakelands#797). A vector field the lake asks an index
  for is the exception: there a page is the index's answer at the depth
  that page asks for, and the pages of one query are not promised to be
  parts of one sequence
  (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
  **A fused tie** is broken by the key
  answered, the one the record bears at the read's position. A fused
  list is computed whole for every page, to the lake's `max_limit`, so
  no cut of an engine's list falls inside it; and the key answered
  belongs to the read's position, which a cursor keeps, so a re-key that
  lands between two pages cannot move a tied record from one page to the
  other. Tying by the head's key would let it.
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
  answer comes out in this order. A vector field the lake asks an index
  for answers the index's set, still in this order; which records are in
  it is the index's (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
