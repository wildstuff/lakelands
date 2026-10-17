# 025 — A search and an order are decided once, for every engine

## What is decided

This record holds what a search answers and in what order anything is
answered. It holds several rules and is as long as they need. 029, 043
and 046 point here: each maps the sections it held to the headings
below.

### Engines find candidates, and the shared functions decide which of them answer, in what order, with what score

An engine's own indexes only find candidates. The shared functions,
which every engine calls, decide which of them answer the engine's
caller, in what order and with which score
(`docs/decisions/031-engines-share-one-rulebook.md`); the store then
applies its reader's grants and fuses the rankings, as the rules below
say. The one case where an engine's index decides which records answer
is a vector field the lake asks an index for, and the answer says so.

### Text is tokenised by the store's one pipeline, so a query matches the same records on every engine

Engines do not tokenise text for full-text search themselves. The store
owns one token pipeline — split into words, lower-case, stem by the
language's BCP-47 tag, fold diacritics — and every engine applies it to
the text it indexes and to the words it is asked for. The same query at
the same position matches the same set of records on every engine, even
though each engine's index holds different bytes.

A new language, or a change to how words are formed, is a change to that
one pipeline, never to a single engine.

### A search limited to named fields matches, scores and ties over those fields only

A lexical or hybrid search that names `fields` matches, scores and ties
over the text of those searchable fields only, and that set and order
are the same on every engine. An engine's full-text index need not know
which field a token came from: the SQLite and PostgreSQL engines draw
their candidates from their whole-record index, a superset of the hits,
and the one reference ranking (`lakelands_core::engines::lexical`)
keeps the records whose named fields match before it cuts the ranking
at `k`. An index per field would narrow earlier, at the price of a
different projection on both engines; it is a later, additive change if
a workload ever asks for it.

### A lexical hit's set, order and score are the same on every engine

One function ranks a lexical search on every engine
(`lakelands_core::engines::lexical`): a hit scores each distinct query
token its text holds, at the weight of the heaviest searched field
holding it, summed in the tokens' order; best first, ties by key. A
field the type's setting does not weigh weighs 1 (below), so a type
that weighs none scores the count of the tokens held, as it always has.
So the set, the order and the score are the same on every engine. A
weight is above 0, so it never changes which records are hits, only
their order and scores. The suite holds it: a
search limited to named fields answers the same keys and scores through
the doors and at the engine seam
(`a_search_limited_to_named_fields_matches_and_ranks_in_those_only`,
`lexical_over_named_fields_is_the_pipelines_over_those_fields`), ranked
search answers best first and equal scores in key order
(`search_is_ordered_by_score_then_key`), and a fused search answers its
keys and the scores' bits on every engine
(`every_page_of_a_hybrid_search_reads_one_ranking`,
`a_search_fuses_its_rankings_each_by_its_weight`).

What is not promised is the function across releases: a later release
may score otherwise, on every engine at once and with the suite's rows
changed with it. A caller compares scores within one answer, and not
across releases.

A token counts once, at its heaviest field, and not once a field it is
in. Summed over the fields, a word in both a title and a body would
score 2 where it scores 1 today, and every lake that weighs nothing
would rank otherwise. BM25F, a sum over fields scaled by their lengths
and the corpus's frequencies, was not taken: the one function holds no
statistics of the lake, and keeping them would put a second structure,
changed at every write, into every engine's projection. The engines'
own rankers, `bm25()` in SQLite and `ts_rank` with its four `setweight`
classes in PostgreSQL, score differently from each other and from the
memory engine, and four classes cannot hold the weights a host writes.
Neither engine's index knows which field a token came from; the
function reads each candidate's fields from the record, so a weight
changes no projection.

### A vector ranking is the cosine, best first, exact to the bit

Which records are the `k` nearest depends on how nearness is measured,
so the measure is part of the contract rather than an engine's choice:
every engine scores by the cosine of the angle between the query and a
stored vector, a vector of zeros scores 0, and equal scores are ordered
by key (`docs/seams.md`, `ranked`). The answer is best first by that
score and then by key. Every engine ranks exactly, by one bounded
selection (`lakelands_core::engines::TopK`): a search keeps its best `k`
while it scans the field, never the field, and answers the reference
ranking's keys, order and scores to the bit. A field the lake asks an
index for answers the index's set instead, ranked by the same cosine,
and says `ranked_by: index`; everything else answers the reference.

A record's vector is not held to a direction. A record may have nothing
to say along a field, and a vector of zeros scores 0 against every
query. Its components must be finite 32-bit floats, as every value's
JSON form says (`docs/log-format.md` §5).

### A restricted record is a candidate, and the store applies the grants to the hits

An engine's full-text and vector candidates include restricted records
(retired ones are excluded), and the store applies the principal's grants
to the hits, as it does for `find` and `get`. A principal holding the
`restricted` grant finds a restricted record by search; one without it
never sees it.

### Several rankings are fused by weighted reciprocal rank, among the hits the read may see

A hybrid search that has a query vector ranks two lists, the lexical one
and the vector one, and fuses them by reciprocal rank: every record a
list ranks gains `weight / (60 + rank)` from it, rank counted from 0,
best first. The lexical list's gain is added first, then the vector
list's. Equal sums are ordered by key.

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
  is a page of that one ranking while no write lands between the pages:
  each pull ranks by the head as it stands (below). An indexed field
  pages by the index's own rule (below).
- **A fused list is read to its window.** The store reads the lists in
  growing batches: a first pull of `max(4 · (offset + k + 1),
  max_limit)` candidates, and four times as many while a list is
  neither whole nor holds `max_limit` records the read may see. Where
  hidden candidates leave a list short of `max_limit` when the first
  page could already be ranked, the list is read on to its window or
  its end. So, while no write lands between them, every page of a
  search answers the same counts, as it answers one ranking. The reads
  it adds fall only there.
- **A vector search names further fields in `also`.** Each is a vector
  field of the type with its own query vector. Every field is ranked by
  the one cosine, among the hits the read may see and to `max_limit`, as
  above, and the rankings are fused: `field`'s first, then `also`'s in
  the order named. A record without a vector in a field gains nothing
  from that field. The answer says `vector`, or `index` when any named
  field is asked for an index.
- **Every ranking has a weight.** A ranking adds `weight / (60 + rank)`.
  A vector search carries `weight` for `field` and one per entry of
  `also`; a hybrid search carries `text_weight` for its words and
  `weight` for its vector. A weight the search does not give is the
  type's setting's, and 1 where the setting names none (below); with
  every weight at 1 the sum is `1 / (60 + rank)` a list, to the bit.
- **A weight is above 0 and at most 1 000 000.** Anything else is
  refused as invalid, naming the field (or `text_weight`). So is a
  `weight` on a vector search that names no other field, and a field
  named twice. These are judged before the principal's rights: they say
  nothing the caller did not write.
- **One list keeps its own score.** A lexical search, a vector search
  of one field, and a hybrid search that falls back to its text are not
  fused. They answer their own scores: a vector search of one field
  answers its cosines.
- **Rights.** Each named vector field is probed as the first is (024):
  the fields it is embedded from, then the field itself, in the order
  named. A field the principal may not know of is answered as
  undeclared.

A host that keeps more than one vector per record, a title's and a
body's, wants one search over both and wants to say which counts for
more. And a hybrid search may want its words to count for more than its
vector, or less.

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
same records differently, and a cursor could repeat or skip a record.
A depth fixed by the lake gives one ranking whatever the page, while
no write lands between the pages. `max_limit` is the bound
the lake already places on a page.

Reciprocal rank, and not a sum of scores, because the scores of two
rankings spread differently. Weights apply to ranks and not to scores
for the same reason. The cosines of two fields spread differently:
short titles may all lie near 0.8 and long bodies near 0.3, so in a
weighted sum of scores the spread would decide, not the number the
caller wrote. A weight on a rank means what it says on every engine.

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

### A type weighs its searches by a setting

A search could weigh its rankings, and only a search could, so a host
that always wanted one field to count more said so in every request. The
setting `search_weights.<type>` says it once:

```
{"fields": {"va": 0.9, "vb": 0.1}, "text_fields": {"title": 4, "body": 0.5}, "text_weight": 0.5}
```

A host whose types give a title more weight than a body could say so
nowhere, so a search for an invoice's number ranked a record naming it
in its title no higher than one mentioning it in a note.

- **It weighs what a search does not.** A fused search that gives a
  vector field no `weight` takes the setting's `fields` weight, and a
  hybrid search with no `text_weight` takes the setting's; a weight the
  search gives wins, and a ranking neither names weighs 1. Each fused
  axis names the weight it ranked by. A search of one axis is not fused,
  and `fields` and `text_weight` change nothing there. A restricted setting's value
  shows only as the weight of an axis its reader is shown, as any
  computation uses a restricted setting's value (`docs/seams.md`).
- **It is a setting, not a member of the field,** because a weight is how
  a host wants its searches ranked today, not what the field is: it
  changes without a migration, and a read as of a position before it
  ranks without it.
- **It says nothing a search cannot say.** It names declared vector
  fields the writer is shown in `fields`, declared searchable fields
  the writer is shown in `text_fields`, and every weight is one a search
  may give, above 0 and at most 1 000 000. Anything else, an unknown
  member, an empty object or empty `fields` or `text_fields`, is
  refused when it is written, naming the member; a field the writer is
  not shown is refused as one the type does not declare, before its
  weight is judged.
- **`text_fields` weighs every ranking of text,** a lexical search's
  and a hybrid search's lexical half alike, and its fallback to the
  text: it weighs a match by the field it is in, inside the ranking,
  and not a ranking against another, so it is read where nothing is
  fused too. A hybrid search's lexical half is ranked by it before the
  fusion, which reads that half's ranks. The axes do not name it: it
  is the type's, not an axis's. A field is weighed only where the
  search reads it, under its name at the head: a field it does not
  name, or whose reader is not shown it, brings no weight, and no
  weight is scaled by another's, so a field its reader is not shown
  moves no score of the fields it is (lakelands#1230).
- **A weight a search could not give weighs nothing.** The store reads
  the setting through the same bound it is written under, so a log
  that came another way, by an import, cannot make a search rank by a
  weight no write could set.
- **It is harmless when the type changes.** A field it names that a
  migration removed, or that is no longer a vector, is named by no
  search and weighs nothing; the setting is not rewritten, so a field
  declared again under that name takes its weight again. The rule that
  carries a grant through a field's rename (lakelands#527) does not reach
  a setting: a host that renames a weighed field writes the setting
  again.

### A search says what each axis brought

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
- **A search of one axis lists it without a count.** It reads until its
  page and one more are found, so a count would change with the page.
  Its hits and its cursor say what it found.
- **A page continued by a cursor computes its counts again.** A search's
  cursor holds an offset and the first page's position, and each page
  reads its records there and ranks again by the head, so the cursor
  carries no count.

A hit's score by axis is not given: that answers why a hit ranks where
it does, a question of its own.

### Every query vector a search names has a direction

A vector search along `(0, 0)` answered every record at a score of 0,
in key order, and a hybrid search fused that key order as if it were a
ranking. Near a vector of no direction is nothing. And a query vector
of `(1e300, 0)` is infinite once read as the 32-bit floats a search
ranks by, so every hit scored `NaN`.

- **Every query vector a search names has a direction:** each component
  a finite 32-bit float, and one at least not 0. That holds for
  `vector`, each entry of `also`, and a hybrid search's vector. A JSON
  number is read as a 32-bit float first, so `1e-200` is 0 and `1e300`
  is not finite.
- **Anything else is refused as invalid, naming the field,** with the
  other faults of its mode (a weight out of its range, a `weight` on a
  vector search that names no other field, a field named twice), before
  the principal's rights: "`v`'s vector has no direction: every
  component is 0", or "has a component that is not a finite number".
- **A vector the embedding backend makes for a hybrid search is held to
  the same rule,** and one without a direction makes the search
  unavailable, as a backend that fails does: the caller did not write
  it.

A query vector is always the caller's: the store makes no model call of
its own accord. A hybrid search without one, over a field a model
fills, has its text embedded by the bound backend, or falls back to its
text where none is bound. So a vector with no direction is the caller's
mistake, and a refusal that names the field says so. An axis that
ranked nothing would answer no hits for a lone search, which reads as
"nothing is near". The rule is exact: a cosine over finite 32-bit
floats, summed in doubles, neither overflows nor underflows.

### A query vector may name its model, and over an embedding it must be the field's

A vector of the right length made by another model was ranked by the
cosine across two spaces, and answered as any other. So each vector a
search brings, `vector`, an entry of `also` and a hybrid search's, may
name its `model`.

- **Over a field declared as an embedding,** a `model` other than the
  field's `embed.model` is refused as invalid before anything is
  ranked, naming both (`query_model`). It is judged by the head's
  declaration, as the length is, after the vector's length, and after
  the caller is judged to see the field, so a caller kept from the field
  is answered as for any vector and told no model.
- **Over a vector field that is no embedding** the lake knows no model,
  so `model` is not judged; the field's axis names it, as it names it
  wherever it was given.
- **A vector without `model`** is ranked as before.
- **A hybrid search that brings no `vector`** takes no `model`: the
  lake makes that vector of the field's own model, so a `model` there
  names nothing the caller brought. It is refused by the search's shape
  alone (`model_without_vector`), with the other misfits and before any
  field is judged for the caller, so the answer tells nothing of the
  field.

### A vector ranking reads a vector field

A vector search over a text or a number field answered no hits and said
nothing, and so did an `also` term over one. A hybrid search over such a
field ranked by its text alone, yet said `hybrid`. A hybrid search
without a vector over a vector field no model fills had its text
embedded by whatever backend was bound, and ranked the result against
vectors its writers made, in a space the lake knows nothing of; over
one of other dimensions it spent the call and was then refused.

- **A vector ranking reads a vector field.** A vector search, each
  `also` term and a hybrid search, with a vector or without, whose field
  is not of kind `vector` is refused as invalid on that field, naming
  its kind (`search_not_vector`). The caller named a ranking, and an
  answer that drops it says something the store did not do.
- **A hybrid search without a vector reads a field a model fills.** Over
  a vector field without `embed` it is refused as invalid on that field
  (`search_needs_vector`), whether or not a backend is bound: the fault
  is in the request read against the declaration, not in the host's
  configuration. Its writers chose the space; the search brings its own
  vector there.
- **Both come after the caller's rights and before any embedding
  call.** A hidden field answers as an absent one and a masked one as
  not granted, as lakelands#741 orders them; a refused search costs no
  call. A field declared with `embed` keeps its comparison with the
  bound backend and its fallback to the text where none is bound, and a
  caller's vector over a plain vector field is ranked by the one cosine.

### A field the lake asks an index for answers the index's set, and says so

A vector search reads every vector of its field and answers the exact
ranking, unless the lake asks for an index on that field. A field the
lake asks an index for answers **the index's set**: the candidates the
engine's index returns, ranked by the one cosine, ties by key. That set
may miss a neighbour the exact ranking finds. The answer says so, and
how often it misses is held by the conformance suite, per engine.

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
  and says `index` too, judged on the head's declaration of the field,
  under the name it bears there. `describe_type` names the fields the
  setting asks for at the position read, as it names `sorted` under 041.
- **Every other field answers the reference.** A field nobody asked an
  index for answers the exact ranking, byte for byte, on every engine
  (the suite's
  `a_vector_search_answers_the_brute_force_ranking_to_the_bit`).

A scan's time grows with the field. lakelands#214 held what a search
keeps to `k` entries, but a search over 100 000 vectors still reads
100 000 vectors. Neither sqlite-vec nor pgvector has an exact index
that avoids that read: sqlite-vec's `vec0` is itself a scan, and
pgvector's indexes, HNSW and IVFFlat, are approximate. A lake that needs
search in less than linear time needs an approximate index, and an
approximate index answers a set that depends on its graph.

Lakelands does not give a different answer per engine without saying
so. So the approximate answer is never the default: the lake asks for
it, per field, in its log, and every search it touches says `index`.
What an agent cannot see is how good the set is; the suite holds that,
as recall against the reference the scan still computes, on every
engine that runs it.

### PostgreSQL keeps the index, and uses it only while it describes the head

- **PostgreSQL** keeps pgvector's HNSW index in the server: the unit
  vector of every vector of the field, `v / |v|` taken in `f64` and
  rounded to `f32`, in a column beside it, and one partial index per
  field over it by L2 distance, which on unit vectors is cosine's order
  (`m = 16`, `ef_construction = 64`, pgvector's defaults). A search asks
  it for as many candidates as the store asks the engine for and one
  more, with `ef_search` at that number, at least 200 and at most 1 000,
  and iterative scans past that. A vector of zeros has no direction
  and is never a candidate of an indexed field, whether the index
  answers the search or the exact scan does: answered by the scan
  alone, it would show a reader that the field is smaller than the
  pull, and so how far the field goes beyond what the reader is shown.
  A field without the setting scores it 0, and so does a field the
  engine has no index to read for: on a server without pgvector, or
  while the field's mark is behind the head. Neither turns on the pull
  or on the field's size. Which way answered still shows by a neighbour
  the index misses and the scan finds; that is left to lakelands#870.
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

### Under an index a tie at the boundary is pulled deeper

The index hands out equal distances in its graph's order, not the keys'.
Cut at `k` and then ordered by key, a group of equal scores larger than
the pull answered some of its records twice across two pages and others
never. So a search asks the index for one candidate more than it
answers, and while that one scores as the last one answered it asks for
twice as many: the members of the tie the index hands out at that depth
are all pulled before the keys cut it, and come once and in key order
at that depth.

That is not the group whole. The index often hands a group out in part
and goes on with farther vectors: of 30 vectors alike among 400 others
the first 65 candidates held between 2 and 30 over sixteen builds, and
of 700 alike put after the build beside a thousand others, paged by ten
and by a hundred, 21 hits came twice and 21 never. The pull is then not
short and no tie is seen cut; a member left out is a neighbour the
index misses, as any other, and a deeper pull may bring it. Beside
those thousand others, with the index built over the group and no
`where`, a group of every size from 2 to 4 097 was answered exactly;
the 30 among 400 were built over too. What an index hands out of a
group moves with its build and with what stands beside it, and no rule
is drawn from either measure.

Where the first pull cuts no tie the answer is the first `k` candidates
the index handed out, and the one more only showed it; from 200 to 999
candidates that pull is searched one candidate wider than a pull of `k`
was. After a deeper pull the answer is the best `k` of all it pulled. A
tie still cut at 4 096 candidates is the exact scan's. Each time the
store asks, the engine's pulls of the index double from the first, what
the store asked for and one, to 4 096: one pull where the first is that
deep, seven where it is 65 to 127, and 4 096 to 12 097 candidates
together for first pulls of 65 to 4 096. 65 is the least for a search
of one ranking. A fused search asks for
`max(4 · (offset + k + 1), max_limit)` at first, so in a lake whose
`max_limit` is under 64 the engine may start below 65, and pull more
often. Where the store asks for 4 096 or more the engine pulls once,
at that depth, and a tie at its boundary is the exact scan's too; a
page past a thousand hits and a filter that hides most candidates ask
for that. The number is the engine's. The deepening looks at scores
alone, never at what a reader may see.

### Under an index a pull that comes back short is answered by the exact scan

The index hands out fewer candidates than it was asked for where the
field holds no more, and also where it holds more. Two such places were
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
  others it went on with farther vectors instead: that pull is not
  short, and the index's answer stands.

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

### Under an index a page is the index's answer at the depth that page asks for

A page is the index's answer at the depth that page asks for, or the
exact ranking where that pull is the scan's. The
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
(`under_an_index_a_tie_it_hands_out_is_paged_once_and_by_key`). A host
that needs a ranked result whole and in order reads it in one page, up to
`max_limit`, or leaves the field without the setting
(`docs/hosting.md`). Pages that are one sequence under an index need
an index that answers the candidates its reader may see, which is
another design (lakelands#870).

### An indexed field's set is stable between writes, and its recall is held at a floor

The same query answers the same set as long as no write and no index
build comes between. On PostgreSQL the set can also move under
`VACUUM`, which repairs the graph after deletes, autovacuum included;
that is the one exception to 040 (maintenance never changes an answer),
and it holds for pgvector-indexed fields only. What holds through
everything is the recall: the suite's
`an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor`
searches a fixed corpus of 4 000 vectors of 32 dimensions around 40
centres with 50 fixed queries, for k = 1, 10 and 100, with and without
a filter, at the head and at an earlier position, and holds the share
of the reference's hits the index finds, averaged over the queries, at
or above the engine's floor: 0.95 at k = 1 and 10 and 0.98 at k = 100
on PostgreSQL, 1 on an engine without an approximate index. PostgreSQL
measured 0.98, 0.97 to 0.98 and 1.00 there over several builds, and at
least 0.996 with a filter. With `ef_search = max(k, 64)`, pgvector's
own default, it measured 0.90 at k = 10, 0.86 at an earlier position: a
query between two clusters is where a narrow search stays in one of
them, so the wider search is the price of the floor. Recall is measured
on this corpus; another lake's vectors may be easier or harder.

### A search at an earlier position ranks by the head

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
indexes they read. A field's kind, its dimensions, and
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
key, the links of a re-keyed end among them. The indexes
hold the head's keys too: a search at an earlier position takes each
candidate back to the key its record bore there, so a record re-keyed
since is ranked by its own text and vectors, and is judged as that
position and the head's holds see it. Equal scores stay
in the order of the keys at the head, which is the order its pages
follow. Its text is ranked as the head holds it, as its vectors
are, so a record retired since is no candidate, by either.

### A search's candidates and its judgement are of one state

The indexes are read as they stand when the engine is asked, and a
search takes its head, the view it judges by, before it asks. A write
that landed in between was in the candidates and not in the judgement:
a re-key moved a held record's text and vector onto a key the earlier
head knew as a visible record's, and that record was answered with the
held record's score; a record restricted and written again was ranked
by a text its reader may not read. So the engine answers
all rankings of one pull from one state of the head and says the
position it is folded through there, and the store judges the
candidates at exactly that position: the keys they bore, the holds and
the reader's grants are read from one view of it.

- A search whose head was taken earlier is made again with the head at
  the position answered, and judges the candidates it has. It asks the
  engine again only where the plan differs there, because a declaration
  of the searched type changed in between.
- **A search at the head is answered at the position its rankings were
  read at.** That is the head its reader was answered from; the answer
  and its cursor name it, and a search for a type that was removed in
  between answers as one made after.
- A search as of a position still reads its records there; only its head
  moves.
- Each pull of a search stands alone: a page is judged from one pull, at
  that pull's position, so no page holds a record of two states.

Reading the indexes again until no write landed in between was the other
way, and was not taken: a ranking over a large field is a scan, an
ingest commits every few milliseconds, and a search beside it would be
void every time, or refused. Holding one snapshot over the whole search
would put every call of a view inside one open transaction on each
engine.

### Every answer of more than one thing has one order

Whenever Lakelands answers with more than one thing, the order is part of
the answer and the same on every engine:

- **Sorted records** follow their sort keys, a record without a value for
  a key after every record with one in either direction, and the record
  key ascending to break ties. Values are compared by one function
  (`lakelands_core::expr::order::cmp_values`): ranked by kind first —
  numbers (and text that reads as one), then instants, then text, then
  booleans, then JSON values — and within a kind as the evaluator
  compares them.
  A position in this order is a row's sort values and its key: a cursor
  carries one, and `find`'s `after` names one in the open, continuing
  strictly after it at the caller's own read position — the head unless
  `as_of` says otherwise — so a cursor is what pins a page set, and
  `after` is what resumes one. A key `latest` orders records by the
  position of the event that made each one's state at the read's
  position; no two records share one, so it never needs the key to
  break a tie. A pattern's rows are projections of a walk, not records:
  they are sorted by their values alone, and a key of them that asks
  for `latest`, `seen_at` or `known_at` is refused.
- **Walks** order what they reach by depth, then type, then key, and links
  by their ends (`docs/decisions/026-the-store-walks-the-graph.md`).
- **Graph measures** order degrees highest first, ties by type and key;
  components largest first, ties by their least type and key, numbered
  from 0 in that order, and a component's records by type and key.
- **Ranked search** answers best first and breaks equal scores by key
  (below).
- **Groups** come in the order of their key's canonical bytes; a
  pattern's aggregate takes no `order_by`.

### Equal scores of one ranking follow the keys at the head, and a fused tie the key answered

Ranked search breaks equal scores by key. The two ties are broken by
different keys, on purpose. **One ranking's** equal scores come in the
order of the keys at the head, where the indexes rank: that is the
order an engine cuts a list at `k` in, and so the order a cursor
follows. A search as of a position before a re-key answers each record
under its key there, and still in that order, so every page is whole;
sorting a page again by the keys answered would answer some of a large
tie twice and some never. A vector field the lake asks an index for is
the exception: there a page is the index's answer at the depth that
page asks for, and the pages of one query are not promised to be parts
of one sequence.

**A fused tie** is broken by the key answered, the one the record bears
at the read's position. A fused list is computed whole for every page,
to the lake's `max_limit`, so no cut of an engine's list falls inside
it; and the key answered belongs to the read's position, which a cursor
keeps, so a re-key that lands between two pages cannot move a tied
record from one page to the other. Tying by the head's key would let
it.

### Aggregates are the store's, except a count alone

Every group and measure — of `find`, of a pattern's rows, of a view —
is computed by the store over the records it reads, decimal-exact, never
by an engine. An aggregate an engine computed would be a second
implementation of the same answer, free to round or order differently;
with one, every engine answers identically by construction.

A view's measure lands in the field the view declares for it, and fails
where that field would not take its value written (023). `find`'s
aggregate and a pattern's rows land in no field: they answer a number
as its digits, in a string, whatever its size.

A count alone is the exception
(`docs/decisions/045-a-table-reads-a-page-and-its-total.md`): `find`'s
`total`, and an aggregate without groups whose measures are all
`count()`, are counted by the engine where it can apply the filter, and
SQLite and PostgreSQL answer with their database's `COUNT(*)` at the
head, when nothing but the flags selects.

## Why

Search is only portable if a lake answers the same question the same way
whatever engine holds it. If each engine tokenised with its own database
features, "Rechnung" would find "Rechnungen" on one engine and not on
another, and a lake moved between engines would silently lose recall.

A page set, a cursor and "the first ten" only mean something if every
engine and every run puts the same records in the same places. An order
left to an engine — a hash map's iteration, an index's physical layout —
looks deterministic until the day it is not, and then a cursor skips a
record or an agent sees a different "top result" for the same question.
The evaluator's own comparison is not a total order across kinds, so a
sort needs a ranking of kinds on top of it; writing that ranking once
keeps every engine's pages identical.

## What it is not

- It does not forbid an engine from using a native full-text index; the
  index holds the pipeline's output.
- It does not forbid an engine from using its own indexes, as long as the
  answer comes out in this order. A vector field the lake asks an index
  for answers the index's set, still in this order; which records are in
  it is the index's.
- It is not a promise that a score means the same in a later release; a
  caller compares scores within one answer.

Of an index asked for:

- **Not the default.** No field is indexed until the lake asks.
- **Not the same set on every engine.** An indexed field's set may
  differ between engines, and between a lake before and after an index
  build. An unindexed field's answer never does.
- **Never a different answer for a field nobody asked for.**
- **Not one sequence across pages.** An unindexed field's pages are
  parts of one order while no write lands between them; an indexed
  field's are not promised to be.
- **Not a seam method.** `nearest` keeps its signature; the engine
  decides from the setting and its mark which path a search takes.
- **Not a declaration.** Nothing in a type says which fields are
  indexed; the setting does, and the log format does not change.
- **Not sqlite-vec.** Its `vec0` is a scan in C over `f32`: a constant
  factor over the scan Lakelands already runs, with a registration that
  needs `unsafe`.

## How it came to be

- The first version: engines apply the store's one token pipeline, so a
  query matches the same records on every engine. In 029: every answer
  of several things has one order, and aggregates are the store's.
- Added: a vector ranking is the cosine, ties by key; a search at an
  earlier position ranks by the head's indexes; a restricted record is
  a candidate.
- 029 added `after`, a sort position named in the open (lakelands#201),
  and `latest` as a sort key (lakelands#196).
- A vector ranking is made by one bounded selection and answers the
  reference to the bit (lakelands#197, lakelands#214).
- 043 was written: a field the lake asks an index for answers the
  index's set, and a vector of zeros is no candidate of it
  (lakelands#197).
- 029 added the order of degrees and components (lakelands#149).
- Added: a search limited to named fields (lakelands#236).
- 046 was written: a hybrid search fuses its lists by reciprocal rank
  among the hits the read may see, each to `max_limit`; before, ranks
  were counted in the engine's raw lists, to a depth that grew with the
  page (lakelands#434).
- 046 added `also` and a weight for every ranking (lakelands#428).
- 043 said when the setting's write builds its indexes: at the fold's
  commit, so that no search waits on a build before then.
- 046 added `axes` and their counts, and replaced the early stop of a
  fused read by the read to its window (lakelands#560).
- 046 added the setting `search_weights.<type>`, which replaced "an
  absent weight is 1" (lakelands#561).
- Added: `search_weights.<type>` weighs a match of the text by the field
  it is in, `text_fields`, each token at its heaviest field, in every
  ranking of text; a lexical hit scored the count of its tokens before,
  which is its score where nothing is weighed (lakelands#1230).
- 046 added: a query vector has a direction (lakelands#696).
- 046 added: a vector ranking reads a vector field, which replaced the
  fallback of a hybrid search over a field no model fills
  (lakelands#743).
- Added: a search ranks by the head's declaration (lakelands#756), a
  migration and a re-key keep the head's indexes by the head's names
  and keys (lakelands#768), and a candidate is taken back to the key it
  bore at the position read, equal scores staying in the order of the
  keys at the head (lakelands#797).
- Added: a search's candidates and its judgement are of one state; in
  029 a fused tie is broken by the key answered, where every tie
  followed the head's keys (lakelands#826).
- 043 added: under an index a tie at the boundary is pulled deeper, a
  short pull is the exact scan's, which leaves a vector of zeros out as
  the index does, and a page is the index's answer at its depth
  (lakelands#827).
- Merged at the rewrite (lakelands#817): the rules of 029, 043 and 046
  moved here, and the title changed with them; the file keeps its name.
  The first rule is new and points at 031.
- Corrected at the rewrite (lakelands#817): "It is not a promise that
  scores are identical across engines. The set of matches is the same;
  ranking within it may differ" (025), "relevance scores" not identical
  (029), "lexical scores differ between engines" and "the lexical
  half's order" each engine's own (046). One function ranks a lexical
  search on every engine, and the suite holds keys and scores; what is
  not promised is the function across releases.
- Corrected at the rewrite (lakelands#817): reciprocal rank was reasoned
  by lexical scores that differ between engines; the reason that holds
  is the one the weights gave, that two rankings' scores spread
  differently.
- Corrected at the rewrite (lakelands#817): 029's `1 / (60 + rank)` and
  its "lexical and vector rankings" are 046's `weight / (60 + rank)`
  over every ranking a search names.
- Corrected at the rewrite (lakelands#817): "a vector search" is not
  fused only where it names one field; "An absent weight is 1" only
  where the setting names none.
- Corrected at the rewrite (lakelands#817): a fused read "stops once the
  first `offset + k + 1` fused hits are each ranked"; it reads every
  list to its window or its end, from a first pull of
  `max(4 · (offset + k + 1), max_limit)`. A search of one axis reads
  "no further than its page and one more": it reads until they are
  found, from a first pull of `max(4 · (offset + k + 1), 64)`.
- Corrected at the rewrite (lakelands#817): "Every page through a cursor
  is a page of one ranking" holds while no write lands between the
  pages; a cursor keeps the position its records are read at, and each
  page ranks again by the head.
- Corrected at the rewrite (lakelands#817): "a hybrid search without
  one falls back to its text or to the bound backend" holds over a
  field a model fills; over any other it is refused.
- Corrected at the rewrite (lakelands#817): aggregates are "never by an
  engine" except a count alone, the engine's since lakelands#380, as
  045 says.
- Corrected at the rewrite (lakelands#817): the heading "vector hits are
  the same on every engine too" but for a field the lake asks an index
  for; the function that compares sort values is
  `lakelands_core::expr::order::cmp_values`.
- Corrected at the rewrite (lakelands#817), of the index's pulls: the
  passage on a vector of zeros says what is left to lakelands#870; a
  fused search "starts at `max_limit` and one" only where `max_limit`
  is the larger of the two; "4 096 to 12 097 candidates" holds for
  first pulls of 65 to 4 096, and a deeper first pull is one pull.
- An order asked for is the order given or it is refused: a pattern's
  rows ordered by `seen_at` or `known_at` came in the order of the
  value, and an `order_by` with a pattern's aggregate was passed over
  (lakelands#901).
- Added: where a measure lands, a view's in its field and an
  aggregate's in none (lakelands#839).
- Added: a query vector may name its model, and over an embedding a
  vector of another model is refused (lakelands#994).
- Added: a search may take a second stage, which ranks the first `top`
  hits of its ranking and ends the answer at them
  (`docs/decisions/067-a-search-may-take-a-second-stage.md`,
  lakelands#993).
