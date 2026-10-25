# 067 — A search may take a second stage, which ranks a window and is not recorded

## What is decided

### A search asks for the stage, and the answer ends at its window

`search` and a pattern's `start` take `rerank: { top, text? }`. The lake
takes the first `top` hits of the search's ranking that its reader may be
answered, after the rights and the `where`, as each fused list is
already read to a window (025, 046), and hands them to the reranker the
host bound. It answers those hits in the order the reranker scores them,
ties in the first stage's order, and each hit's `score` is the
reranker's own.

The answer ends at the window. Continuing past it in the first stage's
order would put two orders in one answer, and a reader could not tell
where one ends. `truncated` says the first stage held more hits than the
window took. A host that wants more asks again with a larger `top`.

- **`top`** is 1 at least and at most the lake's `limits.max_rerank_top`
  (100 unless the location sets it): a reranker is asked about each
  candidate, and a cursor carries the window. A `top` above it is
  refused as invalid, naming the setting. A location that sets it to 0
  is refused when a lake is opened or imported there.
- **`text`** is what the window is ranked against. It defaults to the
  search's own text, so a vector search, which has none, must give it;
  a blank one, given or the search's own, is none.
- **`ranked_by`** adds ` + rerank` to the first stage's name, and `axes`
  ends with `{ axis: "rerank", model, candidates }`, the model of the
  bound reranker and the size of the window.

### The reranker is a seam, and a call is a read

`Reranker` is the ninth seam (`docs/seams.md` §`Reranker`):
`model()` and `rerank(call) -> scores`, one score per candidate, higher
first. It is bound at open (`Seams::rerank`) or later on the open lake
(`LakeStore::bind_rerank`), which is the host's alone. The call is a read
as the query embedding of a hybrid search is: nothing is recorded,
because the ranking is an answer and not a fact of the lake (002).

### The reranker is handed what its reader reads

A candidate is its key and its text: the searchable fields the reader
reads (or the ones the search names), in declared order, joined by a
blank line. The window is taken from the hits the read may be answered,
so a held record is no candidate for a reader without `restricted`, and a
masked or hidden field is never in a text, as it is never in
`searched_fields` (024). The key goes with the text: it is the host's own
seam, and the reader is answered the key anyway.

### The answer is the scored keys, read where the window was

The door reads the window and asks the reranker. The read that answers
then reads at the window's position and on its clock, and answers the
scored keys themselves: each read by its key there and judged for its
reader there, by its grants, its `only`, the search's `where` and the
record's state as they stood at that position. It ranks nothing, so it
is not judged at the head as a search as of a position is (051): the
head's state of a record does not reach it. It never ranks the first
stage again either. So a write that lands after the window was read,
during the reranker's call or later, moves no hit of the answer: one
that would move a record into the window or out of it, retire a scored
record, or move it out of its reader's `only`.

### A cursor carries the scored keys, and the stage is asked once

The first page asks the reranker once. Its cursor carries the stage's
order, each key of the window with its score, best first, and what the
first page said of the first stage: `ranked_by`, `axes` (the reranker's
model among them) and `truncated`, which every page repeats. A later page
reads the carried keys from its offset by key at the cursor's position,
judged there as the first page's were, and asks nothing: paging costs
one call a search. Nor does it embed: a hybrid search sent without a
vector has its text embedded for the window on the first page only, and
its cursor names the read by the mode as its caller sent it, so a later
page is answered whatever the embedding backend answers by then. A
cursor holds at most `top` keys; one that holds more was not handed out
by the lake, and is refused as foreign.

Every carried key is answered but two kinds, whose place in the order
stays empty, so nothing after it is skipped or answered twice, and a page
may hold fewer hits than `k`:

- a record held now, which a reader without `restricted` is not shown at
  any position (006, extended), held since the first page included;
- a key its reader is not answered at the cursor's position, which only a
  cursor handed to another reader, or written by hand, can name.

The cursor is not signed. What it carries is the page's own: a cursor
written by hand names nothing its reader is not answered at that
position, and what it says of the first stage is its writer's.

A search without the stage pages otherwise: its cursor holds an offset
into the first stage's ranking, which it ranks again at the cursor's
position, judged as of then and at the head (051), so a record held,
retired or no longer admitted since closes up the gap and the page after
it begins one hit later.

**Keys, not indexes.** The design of lakelands#993 had the cursor carry
the order as indexes into the window with a digest of the window's keys,
refusing a page whose window no longer matched. Built so, that refusal
told a reader that a record of its window had been held since the first
page, which a search without the stage does not tell, and a cursor handed
to another reader was refused in a lake that held a record from that
reader and answered in a twin that never had it. The keys a cursor
carries are of the window its reader was answered, which its later pages
name anyway.

A pattern started from a reranked search walks the reranked keys the same
way: its door judges the pattern whole first, so a pattern refused for
itself asks the reranker nothing, then asks the stage on the first page,
and its cursor carries the order, at most `top` keys.

### A stage that is not there or fails is a refusal

No reranker bound is `unbound`, naming the seam, once the search is
judged and its first stage read, and before the reranker would be
asked. A reranker that fails, answers other scores than candidates or a
score that is no number is `unavailable`, with the cause, and nothing
is cached. The store never
falls back to the first stage's order for a caller that asked for the
stage: a host that wants the first stage alone asks without `rerank`.

## Why

A cross-encoder judges a query against a text, which the first stage's
score cannot: a one-word query scores every hit alike and the first
stage's order is the key order. A host that reranks one large page outside
the lake loses the lake's cursor past the window, a pattern started from
the reranked order, the window the lake already reads, and the rights
that hold the text it feeds the stage. Inside the lake the stage sees
only what its reader reads and the paging stays one sequence.

## What it is not

- It does not record the ranking, and a replay of the log never calls the
  reranker.
- It does not mix two orders: nothing past `top` is answered.
- It does not change a vector index's paging: under an index the stage
  takes the first `top` hits of its reader's one sequence, as it takes
  the exact ranking's
  (`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`,
  lakelands#1254).
- It is not a fallback: a search without a reranker is refused, not
  answered by the first stage.

## How it came to be

- The second stage, its seam, its window and its cursor, the cursor of
  keys and scores in place of the design's indexes and window digest
  (lakelands#993).
