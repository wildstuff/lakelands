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

## Extended: a ranking has a weight, and a vector search may fuse several fields

A host that keeps more than one vector per record, a title's and a
body's, wants one search over both and wants to say which counts for
more (lakelands#428). And a hybrid search may want its words to count
for more than its vector, or less.

- **A vector search names further fields in `also`.** Each is a vector
  field of the type with its own query vector. Every field is ranked by
  the one cosine, among the hits the read may see and to `max_limit`, as
  above, and the rankings are fused: `field`'s first, then `also`'s in
  the order named. A record without a vector in a field gains nothing
  from that field. The answer says `vector`, or `index` when any named
  field is asked for an index (043).
- **Every ranking has a weight.** A ranking adds `weight / (60 + rank)`.
  A vector search carries `weight` for `field` and one per entry of
  `also`; a hybrid search carries `text_weight` for its words and
  `weight` for its vector. An absent weight is 1, and with every weight
  at 1 the sum is the one above, to the bit.
- **A weight is above 0 and at most 1 000 000.** Anything else is
  refused as invalid, naming the field (or `text_weight`). So is a
  `weight` on a vector search that names no other field, and a field
  named twice. These are judged before the principal's rights: they say
  nothing the caller did not write.
- **A vector search of one field is not fused.** It answers its cosines,
  as before.
- **Rights.** Each named vector field is probed as the first is (024):
  the fields it is embedded from, then the field itself, in the order
  named. A field the principal may not know of is answered as
  undeclared.

Weights apply to ranks and not to scores, for the reason ranks are fused
at all. The cosines of two fields spread differently: short titles may
all lie near 0.8 and long bodies near 0.3, so in a weighted sum of
scores the spread would decide, not the number the caller wrote. A
weight on a rank means what it says on every engine.

A weight of 0 is refused, not read as "leave this ranking out": the
ranking would still be read and its field still probed, and its records
would join the hits at a score of 0. A caller who wants a field ignored
leaves it out. A negative weight is refused because reciprocal rank
over the nearest records cannot say "away from": a far record is in no
ranking, so it would outrank a near one, and the order would depend on
how far the store read. The ceiling keeps every sum finite; weights are
relative, so it takes nothing a caller needs.

A hybrid search does not take `also`: its text is embedded by one
model, and several fields may have several.

## Extended: a search says what each axis brought (lakelands#560)

A hybrid search whose text matched nothing answered as one whose text
matched, only its scores lower. A host could tell the two apart only by
running each half again on its own. So every answer lists the axes that
ranked it, `axes`, in the order the fusion sums them, a vector axis
naming its field.

- **Where a search fuses, each axis is counted.** The count is of the
  candidates the axis handed the fusion *for this reader*: after the
  search's `where` and the rights that decide a hit, and up to the
  lake's `max_limit`. It is never a count of the lake's matches, and a
  host must not read it as one; `total` stays unknown.
- **A fused list is read to its window.** Where hidden candidates leave
  a list short of `max_limit` when the first page could already be
  ranked, the list is read on to its window or its end. So every page
  of a search answers the same counts, as it answers one ranking. The
  reads it adds fall only there.
- **A search of one axis lists it without a count.** It reads no further
  than its page and one more, so a count would change with the page.
  Its hits and its cursor say what it found.
- **A page continued by a cursor computes its counts again.** A search's
  cursor holds an offset and the first page's position, and each page
  ranks again there, so the cursor carries no count.

A hit's score by axis is not given: that answers why a hit ranks where
it does, a question of its own.
