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
score and then by key. Every engine ranks exactly, by one bounded
selection (`lakelands_core::engines::TopK`): a search keeps its best `k`
while it scans the field, never the field, and answers the reference
ranking's keys, order and scores to the bit (lakelands#197). A field the
lake asks an index for answers the index's set instead, ranked by the
same cosine, and says `ranked_by: index`; everything else answers the
reference (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).

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

So a search reads its names, its caller's rights and its `where` at the
position it reads, and its rankings by the head's declaration, whose
indexes they read (lakelands#756). A field's kind, its dimensions, and
whether a model fills it and which are the head's: a query embedded by
the model of the earlier position, or a query vector of its length, would
be ranked against vectors of another space or length. Each field a
ranking names is followed to the name it bears at the head, as 051
follows a type's. A field the head does not declare holds no vectors
there and no model fills it, so a vector ranking over it has no
candidate and a hybrid search without a vector over it is refused as
needing one. Following a name never lets a ranking reach a field its
caller is not shown at the head: one shown at the earlier position and
hidden at the head answers as one the head does not declare, and one
hidden at the earlier position is refused as it is there. A field the
head declares and the earlier position does not is unknown, as every
name is read there. For the head's indexes to hold the head's names, a
migration re-indexes each record of its type by the declaration it puts
in force, and takes out the vectors of the fields it renames or removes;
a re-key takes the vectors of every record whose key it moves to the new
key, the links of a re-keyed end among them (lakelands#768). The indexes
hold the head's keys too: a search at an earlier position takes each
candidate back to the key its record bore there, so a record re-keyed
since is ranked by its own text and vectors, and is judged as that
position and the head's holds see it (lakelands#797). Equal scores stay
in the order of the keys at the head, which is the order its pages
follow (029). Its text is ranked as the head holds it, as its vectors
are, so a record retired since is no candidate, by either.

## Extended: a restricted record is a candidate

An engine's full-text and vector candidates include restricted records
(retired ones are excluded), and the store applies the principal's grants
to the hits, as it does for `find` and `get`. A principal holding the
`restricted` grant finds a restricted record by search; one without it
never sees it.

## Extended: a search limited to named fields

A lexical or hybrid search that names `fields` matches, scores and ties
over the text of those searchable fields only, and that set and order
are the same on every engine. An engine's full-text index need not know
which field a token came from: the SQLite and PostgreSQL engines draw
their candidates from their whole-record index, a superset of the hits,
and the one reference ranking (`lakelands_core::engines::lexical`)
keeps the records whose named fields match before it cuts the ranking
at `k`. An index per field would narrow earlier, at the price of a
different projection on both engines; it is a later, additive change if
a workload ever asks for it (lakelands#236).
