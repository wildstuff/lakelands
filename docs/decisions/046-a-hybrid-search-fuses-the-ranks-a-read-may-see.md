# 046 — A hybrid search fuses the ranks a read may see

## What is decided

A hybrid search that has a query vector ranks two lists, the lexical one
and the vector one, and fuses them by reciprocal rank: every record a
list ranks gains `1 / (60 + rank)` from it, rank counted from 0, best
first. The lexical list's gain is added first, then the vector list's.
Equal sums are ordered by key.

- **Each list ranks only what the read may see.** A record that the read
  leaves out takes no rank. That covers a retired record, a restricted
  one for a principal without the grant, one not valid at `valid_at`,
  and one the search's `where` leaves out. The test is the same one that
  decides whether a record is a hit at all. So the first hit the read can
  see is rank 0 in each list that ranks it, whatever is ranked above it
  for someone else.
- **Each list counts to the lake's `max_limit`.** Only a list's first
  `max_limit` visible records gain anything from it (1 000 by default, the
  same bound a pattern's search start already reads to). The fused
  ranking therefore depends on the query, the position and the read's
  visibility, never on the page asked for. Every page through a cursor
  is a page of one ranking.
- **How far the store reads.** The store reads the lists in growing
  batches. It stops once the first `offset + k + 1` fused hits are each
  ranked by every list that could still rank them, and nothing behind
  them could still reach the last of them. That is when the page and
  its scores can no longer change. Where to stop is a cost, not an
  answer: reading every list to the bound answers the same.
- **One list keeps its own score.** A lexical search, a vector search,
  and a hybrid search that falls back to its text are not fused. They
  answer as before.

## Why

A hybrid score used to count ranks in the engine's raw lists and drop
what the read may not see only afterwards. A restricted record ranked
above a visible one lowered that hit's score from 2/60 to 2/61. Any
caller could choose a text and a vector and read from the score that
something it may not see lies nearer. 024 does not allow a read to
answer that question, and 006 keeps a restricted record from every
principal without the grant. Ranking among the visible hits answers
only what the read may see.

The depth each list was read to also grew with the page:
`max(4 · (offset + k + 1), 64)`. A record past that depth in one list
lost that list's gain, so a first page and a deeper one could rank the
same records differently, and a cursor could repeat or skip a record
(029: every answer has one order). A depth fixed by the lake gives one
ranking. `max_limit` is the bound the lake already places on a page.

Reciprocal rank, and not a sum of scores, because lexical scores differ
between engines (025 promises the same match set, not the same scores).
A sum would rank a hybrid search differently per engine. Ranks are the
same wherever the lists are.

## What it is not

- It does not make the lexical half's order the same on every engine.
  That order is each engine's own within the one match set (025). Only
  the fusion is the same.
- It does not weigh the two halves. That is a separate decision.
