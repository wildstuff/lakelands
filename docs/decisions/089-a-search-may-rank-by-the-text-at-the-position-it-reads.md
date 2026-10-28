# 089 — A search may rank by the text at the position it reads

## What is decided

**Proposed** on lakelands#1541, for 0.21.0. The operator reviews it on
the issue; merging it does not accept it. It needs no log format, no
step and no engine work: the store ranks with what a read at a position
already answers. It extends
`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`,
whose "A search at an earlier position ranks by the head" names this as
a later, additive change, and comes into force with the build, which
rewrites that section where it stands.

A search with `as_of` ranks by the head's text and keeps the records
visible at the position (025). A record whose text held the words then,
and holds them no longer, is no candidate at all. A reader of a document
stored as one record per page, ingested again with its words moved,
cannot learn which page held a word at an earlier position: the page that
held it is never ranked, and the page that holds it now answers with
empty `matches`.

### A search may ask to be ranked by the text it reads

`Search.text_as_of: bool`, serde default `false` and skipped when false.
With it, a lexical search reads its candidates' text at the position it
reads, not at the head:

- the candidates are the records the search's `filter` selects at that
  position, read as `find` reads them there: names, rights, holds and
  keys of that position, as 025 already reads a search's `where`;
- the store tokenises their searched fields with its one pipeline, and
  scores them with the same lexical function as at the head, its
  statistics (the document count, each field's average length and each
  term's document frequency) taken over those candidates;
- `matches` are of the text it ranked, so every hit with a match is a
  record that held the words at that position, and a record that did
  not hold them is no hit.

The order, the ties by key and the pages are 025's. Since the text at a
past position does not change, a search with `text_as_of` answers the
same at every head, and its pages are one sequence whatever lands in
between.

### It asks a bound and an earlier position

It is refused `invalid`, naming the member, before any record is read,
where the search
- reads no earlier position (`at.as_of` absent): at the head, the head's
  text is the position's, and the index already ranks it;
- has no `filter`;
- ranks by a vector, alone or beside the text: a vector at an earlier
  position was made by the model of then, and 025's reasons for ranking
  vectors by the head stand;
- reranks (`rerank`): a second stage ranks the head's text.

Where the filter selects more records at that position than the lake's
new limit `max_text_as_of` (default 10 000, at most 1 000 000), the
search is refused `limit_range`, naming the limit and that the filter
selects more. It is never cut short: a ranking over part of the set
would answer a wrong best page, where the caller's workaround answers
none. The store stops counting at the limit plus one.

### Rights are those of both positions, as 025 already says

A ranking reaches a field on a record only where its reader is shown
the field there both at the position read and at the head (025, 006).
A field hidden on the records a condition selects is judged per record
in the same way. `text_as_of` changes which text is ranked, never who
may see it.

### What it takes to build

Estimated from the nearest siblings: the read of a filter at a position
(`find` with `as_of`) and the shared lexical scorer the memory engine
already ranks with.

- **Product (core crates), est.:** +200 to +300:
  - in `lakelands-api`, the member and its doc and the limit;
  - in `lakelands-store`, the refusals, the bounded read of the filter
    at the position, and the scorer over the candidates' text with
    statistics of their own;
  - the matches from the same tokens.
  The engines and the seam do not move.
- **Specification (core crates), est.:** +250 to +400: the checks below.
- **Outside the core:** the doors carry the member (MCP's `search`
  schema, REST, the CLI's `search --text-as-of`, the client), the
  setting in `docs/settings.md`, `docs/rest.md`, and the regenerated
  `docs/refusals.md` and `docs/conformance.md`.

The checks, on every engine:
1. `search::TEXT_AS_OF_FINDS_THE_TEXT_THEN`: the case of lakelands#1541.
   Page 1 holds `Cover letter` and page 2 `Invoice 4711, total`; a second
   ingest swaps them. A search for `4711` as of before it, filtered to
   the document, with `text_as_of` and `matches`, answers page 2 alone,
   its match in its text then. Without `text_as_of` it answers as today.
   At the head with neither, it answers page 1.
2. `search::TEXT_AS_OF_SAME_AT_EVERY_HEAD`: the same search, its first
   page and its cursor's next, answers the same after further writes to
   those records.
3. `search::TEXT_AS_OF_REFUSED`: without `as_of`, without `filter`, with
   a vector axis and with `rerank`, each is refused `invalid` naming
   `text_as_of`, with nothing read; a filter selecting the limit plus one
   is refused `limit_range` naming `max_text_as_of`.
4. `search::TEXT_AS_OF_HOLDS_BOTH_POSITIONS`: a field shown then and
   hidden at the head, and one hidden then and shown at the head, are
   each neither matched nor ranked.

## Why

025 ranks a dated search by the head because keeping a full-text index
for every earlier state would give each engine a second history of its
largest structure, for a ranking no caller had asked for. A caller now
asks: the page that held a word at a position, in a document whose
words moved.

That caller's search is bounded by a filter (the document's pages), so
it needs no index of the past. The store can read the bounded set at the
position, as `find` reads it, and rank it with the pipeline and scorer
it ranks with at the head. That needs no second history, no engine work
and no log format. The bound is what keeps it cheap: a filter that
selects more than the limit is refused, not scanned.

A lexical predicate in `where` (`text_matches(field, query)`) was the
other shape asked for, and was not taken:
- The function set of the expression grammar is closed
  (`docs/decisions/017-one-expression-grammar.md`). A condition stands in
  grants, sets, triggers and derived fields, which the fold evaluates and
  a replay must reproduce. A condition that tokenises would make the
  pipeline's stemming part of the log's contract, and 025 promises
  no score or tokenisation across releases.
- Kept to `find`'s `where`, it would select without an order or a
  score, and `find` would need `matches` of its own. It would also scan
  the type wherever nothing else bounds it.

## What it is not

- **Not an index of the past.** Nothing is kept per position; the text
  is read and tokenised at the search.
- **Not a vector at a position.** A vector ranking stays the head's
  (025).
- **Not a change of the default.** A search without `text_as_of` ranks
  by the head, as it did.
- **Not a predicate.** The expression grammar gains no function (017).
- **Not a score to compare.** The statistics are the candidates', so a
  score compares only within one answer, as 025 already says of every
  score.
