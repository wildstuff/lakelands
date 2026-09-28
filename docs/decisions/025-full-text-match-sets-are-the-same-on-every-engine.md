# 025 — Full-text match sets are the same on every engine

## What is decided

Engines do not tokenise text for full-text search themselves. The store
owns one token pipeline — split into words, lower-case, stem by the
language's BCP-47 tag, fold diacritics — and every engine applies it to
the text it indexes and to the words it is asked for. The same query at
the same position matches the same set of records on every engine, even
though each engine's index holds different bytes.

A new language, or a change to how words are formed, is a change to that
one pipeline, never to a single engine.

## Why

Search is only portable if a lake answers the same question the same way
whatever engine holds it. If each engine tokenised with its own database
features, "Rechnung" would find "Rechnungen" on one engine and not on
another, and a lake moved between engines would silently lose recall.

## What it is not

- It is not a promise that scores are identical across engines. The set
  of matches is the same; ranking within it may differ.
- It does not forbid an engine from using a native full-text index; the
  index holds the pipeline's output.

## Extended: vector hits are the same on every engine too

The same reasoning holds for the vector index. Which records are the
`k` nearest depends on how nearness is measured, so the measure is part
of the contract rather than an engine's choice: every engine scores by
the cosine of the angle between the query and a stored vector, a vector
of zeros scores 0, and equal scores are ordered by key
(`docs/seams.md`, `VectorIndex`). The answer is best first by that
score and then by key. Every index in `0.1.0` is exact. An approximate
index, when one comes, says so; the store re-ranks its candidates by the
same cosine and the answer says its ranking was approximate, so a caller
always knows which kind of answer it holds. Every engine ranks exactly, by one
bounded selection (`lakelands_core::engines::TopK`): a search keeps its
best `k` while it scans the field, never the field, and answers the
reference ranking's keys, order and scores to the bit (lakelands#197).

## Extended: a search at an earlier position ranks by the head

Search is ranked retrieval, and `as_of` decides what may be seen, not how
the current text or vectors rank. A search at an earlier position ranks
by the full-text and vector indexes as they stand at the head, and keeps
only the records visible at that position; a record that did not exist
then is never a hit, and a record whose text or vector changed since is
ranked by the newer one. Keeping full-text and vector indexes for every
earlier state would make each engine carry a second history of its
largest structures, for a ranking no caller has asked for; if one ever
does, it is a later, additive change.

## Extended: a restricted record is a candidate

An engine's full-text and vector candidates include restricted records
(retired ones are excluded), and the store applies the principal's grants
to the hits, as it does for `find` and `get`. A principal holding the
`restricted` grant finds a restricted record by search; one without it
never sees it.
