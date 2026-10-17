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
  refused as invalid, naming the setting.
- **`text`** is what the window is ranked against. It defaults to the
  search's own text, so a vector search, which has none, must give it.
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

### A cursor carries the order, and the stage is asked once

The door reads the window, asks the reranker, and reads again with its
scores at the window's position and on its clock, so the scores and the
answer are of one window. The first page asks the reranker once. Its
cursor carries the stage's order: each key of the window with its score,
best first, and the reranker's model. A later page reads the first stage
again at the cursor's position, takes from it the hits the carried keys
name, and asks nothing: paging costs one call a search and is stable.

A page answers only what its own window holds. A record of the window
held since the first page, which a reader without `restricted` is not
shown at any position, is not answered, and neither is a key the window
does not hold, for a cursor handed to another reader or written by hand:
its place in the order stays empty, so nothing after it is skipped or
answered twice, and a page may hold fewer hits than `k`. A search without
the stage leaves such a record out the same way. The carried keys are of
the window its reader was answered, which later pages would name anyway.
A pattern started from a reranked search walks the reranked window: its
door asks the stage on the first page, and its cursor carries the order
the same way.

### A stage that is not there or fails is a refusal

No reranker bound is `unbound`, naming the seam, once the search is
judged and its first stage read, and before the reranker would be asked. A reranker that fails,
answers other scores than candidates or a score that is no number is
`unavailable`, with the cause, and nothing is cached. The store never
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
- It does not change a vector index's paging (lakelands#870): the stage
  takes whatever window the first stage gives, indexed or exact.
- It is not a fallback: a search without a reranker is refused, not
  answered by the first stage.

## How it came to be

- The second stage, its seam, its window and its cursor (lakelands#993).
