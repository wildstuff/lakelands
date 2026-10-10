# 043 — An index asked for answers its set

## What is decided

A vector search reads every vector of its field and answers the exact
ranking (025, 029), unless the lake asks for an index on that field. A
field the lake asks an index for answers **the index's set**: the
candidates the engine's index returns, ranked by the one cosine, ties by
key. That set may miss a neighbour the exact ranking finds. The answer
says so, and how often it misses is held by the conformance suite, per
engine.

- **Asked for per field.** The `sys.setting` record `vector_index.<type>`
  holds a JSON array of the type's vector fields. The store refuses, as
  invalid and naming the field, a write of the setting that names a
  field the type does not declare as a vector, or a type it does not
  declare. Retiring the setting, or an empty array, asks for no index.
  It is a setting and not part of the declaration for the reason 041
  gives: it changes what an answer costs and how exact it is, not the
  type's shape, so it needs no new log format and no schema hash.
- **What the answer says.** A search over an asked-for field says
  `ranked_by: index` (vector) or `hybrid (index)` (hybrid, whose vector
  side used it). `describe_type`'s lifecycle names the asked-for fields
  as `indexed`, so an agent sees which searches are approximate before it
  searches. `ranked_by` follows the setting at the head, on every engine,
  since the engines keep their vector indexes at the head: a search at an
  earlier position over an asked-for field is ranked by the head's index
  and says `index` too, judged on the head's declaration of the field, under
  the name it bears there (025). `describe_type` names the fields the setting asks
  for at the position read, as it names `sorted` under 041.
- **Every other field answers the reference.** A field nobody asked an
  index for answers the exact ranking, byte for byte, on every engine
  (the suite's
  `a_vector_search_answers_the_brute_force_ranking_to_the_bit`).
- **What each engine keeps.**
  - **PostgreSQL** keeps pgvector's HNSW index in the server: the unit
    vector of every vector of the field, `v / |v|` taken in `f64` and
    rounded to `f32`, in a column beside it, and one partial index per
    field over it by L2 distance, which on unit vectors is cosine's order
    (`m = 16`, `ef_construction = 64`, pgvector's defaults). A search asks
    it for as many candidates as the store asks the engine for, with
    `ef_search = max(k, 200)`, at most 1 000, and iterative scans past
    that. A vector of zeros has no direction and is never a candidate.
  - **SQLite and memory** keep no such index. They accept the setting,
    say `index` and `indexed`, and answer the exact ranking: the index's
    set is the reference there. A SQLite lake that needs an index moves
    to PostgreSQL by export and import (lakelands#215).
- **Only while it describes the head.** On PostgreSQL each indexed field
  has a mark (`vindex.<type>.<field>`) the fold keeps at `folded`, as the
  order index's (041). A search uses the index only while the mark equals
  `folded`; otherwise it scans, and answers the reference. The setting's
  write builds its type's indexes in the fold's transaction, at its
  commit, so no search waits on a build before then; a
  redeclaration of the type builds them again; a writer's open builds
  those whose mark is behind; `rebuild` and `clear` take them with the
  projection.
- **A server without pgvector** (or with a version before 0.8, which has
  no iterative scans) keeps no index: nothing is marked and every search
  answers the reference. A writer's open tries `CREATE EXTENSION IF NOT
  EXISTS vector` and, when the lake asks for an index it cannot keep,
  says once on standard error why. It never refuses the lake.
- **Stable between writes, and at the floor always.** The same query
  answers the same set as long as no write and no index build comes
  between. On PostgreSQL the set can also move under `VACUUM`, which
  repairs the graph after deletes, autovacuum included; that is the one
  exception to 040 (maintenance never changes an answer), and it holds
  for pgvector-indexed fields only. What holds through everything is the
  recall: the suite's
  `an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor`
  searches a fixed corpus of 4 000 vectors of 32 dimensions around 40
  centres with 50 fixed queries, for k = 1, 10 and 100, with and without
  a filter, at the head and at an earlier position, and holds the share
  of the reference's hits the index finds, averaged over the queries, at
  or above the engine's floor: 0.95 at k = 1 and 10 and 0.98 at k = 100
  on PostgreSQL, 1 on an engine without an approximate index. PostgreSQL
  measured 0.98, 0.97 to 0.98 and 1.00 there over several builds, and at least 0.996 with a filter.
  With `ef_search = max(k, 64)`, pgvector's own default, it measured 0.90
  at k = 10, 0.86 at an earlier position: a query between two clusters
  is where a narrow search stays in one of them, so the wider search is
  the price of the floor. Recall is measured on this corpus; another
  lake's vectors may be easier or harder.

## Why

A scan's time grows with the field. lakelands#214 held what a search keeps to
`k` entries, but a search over 100 000 vectors still reads 100 000
vectors. Neither sqlite-vec nor pgvector has an exact index that avoids
that read: sqlite-vec's `vec0` is itself a scan, and pgvector's indexes,
HNSW and IVFFlat, are approximate. A lake that needs search in less than
linear time needs an approximate index, and an approximate index answers
a set that depends on its graph.

Lakelands does not give a different answer per engine without saying
so (025, 029). So the approximate answer is never the default: the lake
asks for it, per field, in its log, and every search it touches says
`index`. What an agent cannot see is how good the set is; the suite holds
that, as recall against the reference the scan still computes, on every
engine that runs it.

The index is kept in PostgreSQL only. One installation runs one engine;
the memory engine is for tests and small lakes, and a SQLite lake is a
file on one machine where a scan of the field is a read of the file. A
second index implementation in Rust for those two would buy a sub-linear
search for lakes that rarely need one, and a second graph to keep
consistent across processes and crashes. A SQLite lake that grows into
the need moves to PostgreSQL.

The unit vectors are the index's, not the answer's: the candidates are
ranked by the one cosine over the stored vectors, as every other search
is, so the index decides which records are candidates and never how
they score or in which order ties fall.

## What it is not

- **Not the default.** No field is indexed until the lake asks.
- **Not the same set on every engine.** An indexed field's set may
  differ between engines, and between a lake before and after an index
  build. An unindexed field's answer never does.
- **Never a different answer for a field nobody asked for.**
- **Not a seam method.** `nearest` keeps its signature; the engine
  decides from the setting and its mark which path a search takes.
- **Not a declaration.** Nothing in a type says which fields are
  indexed; the setting does, and the log format does not change.
- **Not sqlite-vec.** Its `vec0` is a scan in C over `f32`: a constant
  factor over the scan Lakelands already runs, with a registration that
  needs `unsafe`.
