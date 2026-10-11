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
    it for as many candidates as the store asks the engine for and one
    more (below), with `ef_search` at that number, at least 200 and at
    most 1 000, and iterative scans past that. A vector of zeros has no
    direction and is never a candidate of an indexed field, whether the
    index answers the search or the exact scan does (below): answered
    by the scan alone, it would show a reader that the field is smaller
    than the pull, and so how far the field goes beyond what the reader
    is shown. A field without the setting scores it 0, and so does a
    field the engine has no index to read for: on a server without
    pgvector, or while the field's mark is behind the head. Neither
    turns on the pull or on the field's size.
  - **A tie at the boundary is pulled deeper (lakelands#827).** The
    index hands out equal distances in its graph's order, not the keys'.
    Cut at `k` and then ordered by key, a group of equal scores larger
    than the pull answered some of its records twice across two pages
    and others never. So a search asks the index for one candidate more
    than it answers, and while that one scores as the last one answered
    it asks for twice as many: the members of the tie the index hands
    out at that depth are all pulled before the keys cut it, and come
    once and in key order at that depth. That is not the group whole.
    The index often hands a group out in part and goes on with farther
    vectors: of 30 vectors alike among 400 others the first 65
    candidates held between 2 and 30 over sixteen builds, and of 700
    alike put after the build beside a thousand others, paged by ten
    and by a hundred, 21 hits came twice and 21 never. The pull is then
    not short and no tie is seen cut; a member left out is a neighbour
    the index misses, as any other, and a deeper pull may bring it.
    Beside those thousand others, with the index built over the group
    and no `where`, a group of every size from 2 to 4 097 was answered
    exactly; the 30 among 400 were built over too. What an index hands
    out of a group moves with its build and with what stands beside it,
    and no rule is drawn from either measure. Where the
    first pull cuts no tie the answer is the first `k` candidates the
    index handed out, and the one more only showed it; from 200 to 999
    candidates that pull is searched one candidate wider than a pull of
    `k` was. After a deeper pull the answer is the best `k` of all it
    pulled. A tie still cut at 4 096 candidates is the exact scan's.
    The pulls double from the first to 4 096: one pull where the first
    is that deep, seven where it is 65 to 127, and 4 096 to 12 097
    candidates together from a first pull of 65 on. That is the least
    a search asks, but a fused one in a lake whose `max_limit` is under
    64, which starts at `max_limit` and one and so pulls more often.
    So is a tie at the boundary of a pull
    that is already that deep, which a page past a thousand hits and a
    filter that hides most candidates ask for. The number is the
    engine's. The deepening looks at scores alone, never at what a
    reader may see.
  - **A short answer is not the field's end (lakelands#827).** The index
    hands out fewer candidates than it was asked for where the field
    holds no more, and also where it holds more. Two such places were
    measured:
    - **A large group of identical vectors.** pgvector reaches a part of
      it, however many are asked for (measured on a field of 8
      dimensions that holds the group alone):

      | vectors alike | index built over them | put after the build |
      |---|---|---|
      | 70, 200, 330 | all | |
      | 340 | 330 | |
      | 700 | 330 | 680 |
      | 3 000 | 1 175 | 2 878 |

      There it hands out that part and ends, short of the pull, and so
      it did with 400 other vectors beside the 700. Beside a thousand
      others it went on with farther vectors instead (above): that pull
      is not short, and the index's answer stands.

    - **A pull past `hnsw.max_scan_tuples`,** where an iterative scan
      ends. A search whose filter keeps few records pulls that deep:
      over 25 000 vectors, a `where` that kept the five farthest was
      answered no hit. A search sets the number itself, to pgvector's
      default of 20 000, so that where a pull comes back short does not
      move with the server's value of it. pgvector also ends such a scan
      at its memory limit (`hnsw.scan_mem_multiplier` times `work_mem`),
      which stays the server's; a pull that ends there is short too, and
      the scan's.

    In both the store took the short pull for the end of the field and
    said there was no more. The engine cannot tell the cases apart by
    what came, so **a pull that comes back short is answered by the
    exact scan**. A search the index fills pays one candidate more. A pull
    larger than what the index hands out reads the field a second time:
    every search of a field smaller than its pull, and every search
    whose filter drives the pull past the field, which has read the
    field through the index by then. Measured: the search of
    the 25 000 answers its five in 2.7 s where it answered none in 1.8
    to 3.0 s; the same over 5 000 vectors takes 813 ms for 694 ms; a
    search of ten without a filter takes 7 to 12 ms either way. The
    engine looks at how many candidates came, never at who reads.
  - **SQLite and memory** keep no such index. They accept the setting,
    say `index` and `indexed`, and answer the exact ranking: the index's
    set is the reference there. A SQLite lake that needs an index moves
    to PostgreSQL by export and import (lakelands#215).
- **A page is the index's answer at the depth that page asks for,** or
  the exact ranking where that pull is the scan's. The
  store pages a search by asking the engine for more candidates than
  the page before did: the pages before it, the page, and what a filter
  or the grants hide. Under an index two pulls of different depth are
  two answers: the deeper one is searched wider and may find a
  neighbour the shallower one missed. **The pages of one query are so
  not promised to be parts of one sequence: a hit may come twice or be
  left out** where the index answers another set at another depth.
  Measured on the suite's corpus, each of the 50 queries paged by ten to
  its end: of 206 000 hits 14 came a second time and 14 records never,
  in two of the queries and none of them in a tie. The count moves with
  the build of the graph: within the first twelve pages 14, 26, 40 and
  43 hits came a second time on four builds here (the 40 with 67
  records left out), and 26 with 53 left out on the reviewer's. The
  suite's check prints it and judges
  nothing. What the suite holds is a tie the index hands out at each
  page's depth, each member once and in key order, and a group it
  hands out in part
  (`under_an_index_a_tie_it_hands_out_is_paged_once_and_by_key`). A host that needs a
  ranked result whole and in order reads it in one page, up to
  `max_limit`, or leaves the field without the setting
  (`docs/hosting.md`). Pages that are one sequence under an index need
  an index that answers the candidates its reader may see, which is
  another design (lakelands#870).
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
- **Not one sequence across pages.** An unindexed field's pages are
  parts of one order (029); an indexed field's are not promised to be.
- **Not a seam method.** `nearest` keeps its signature; the engine
  decides from the setting and its mark which path a search takes.
- **Not a declaration.** Nothing in a type says which fields are
  indexed; the setting does, and the log format does not change.
- **Not sqlite-vec.** Its `vec0` is a scan in C over `f32`: a constant
  factor over the scan Lakelands already runs, with a registration that
  needs `unsafe`.
