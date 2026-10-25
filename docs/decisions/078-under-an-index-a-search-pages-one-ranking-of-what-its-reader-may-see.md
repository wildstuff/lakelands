# 078 — Under an index a search pages one ranking of what its reader may see

## What is decided

**Accepted** on lakelands#1254, by the operator on 2026-10-25, for
0.19.0, built from `develop` as one fix and measured on the build under
"Why". Under an index a search answers one sequence, and nothing its
reader may not see shapes what it is answered: not the candidates the
engine and the store count, and not the index's graph the candidates
come from. It has three parts, which ship as one fix:

- **the stream** makes the pages of one query parts of one sequence, and
  takes away every count made over candidates a reader may not see;
- **the engine's judgement** takes away most of the cost, and adds two
  fields to the `ranked` seam;
- **a graph per class of reader** streams each reader from a graph that
  holds only what its class may see, and adds a third field. On
  PostgreSQL each graph is a table of its own, and the layout becomes 7.

It changes no setting and no pgvector minimum. It rewrites the rules of
`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`
named under "What it changes in 025", and the records named under "The
layout is 7".

### Under an index a field is ranked as one stream hands it out

While its graph does not change, a field the lake asks an index for is
ranked, for one query and one class of reader (below), by one sequence:
every vector the class's graph hands out in its stream, ranked by the
one cosine with equal scores by key, and after them every other vector
of the field the class may see but a vector of zeros, in the same order.
A pull of `k` answers the first `k` of that sequence, so it is the first
`k` of every deeper pull, as a pull of the exact ranking is.

- **A pull reads only what decides its first `k`.** The stream hands its
  vectors out each as far as or farther than the one before, by the
  index's distance `d`. The engine reads the stream's first `k` admitted
  rows, then every row after them whose `1 − d²/2` is within the margin
  of the `k`th row's, and ranks those by the cosine. A row past the
  margin scores below each of the first `k`, so it is not among the best
  `k` at this pull or at a deeper one.
- **The margin is `(dimensions + 8) / 2²²` of the cosine.** pgvector
  keeps each unit vector rounded to single precision and sums `d²`, at
  most 4, in single precision. The cosine is taken in double from the
  vectors the lake holds. Over `n` dimensions, `1 − d²/2` is within
  `(n + 4) / 2²³` of the cosine: at most `(n − 1) / 2²³` from the sum,
  the rest from rounding each term and the unit vectors. A cut compares
  two rows, so it needs more than twice that error.
- **The store pages it as it pages the exact ranking.** It cuts a page
  from a pull by its offset, and pulls four times deeper while the
  candidates hidden from its reader leave the page short, as today.
  Every pull is a prefix of the one sequence, so the depth no longer
  decides the answer. The pages of one query are cut from the hits its
  reader may see of that sequence: each once, none twice, none left
  out. A hidden candidate moves no hit's place; it costs the time of
  reading past it.
- **What the stream does not hand out comes after it.** A strict stream
  drops a vector it finds after a farther one was handed out, and the
  graph may not reach every vector
  (`025 § Under an index the rest of the field follows the stream`).
  Where a pull reaches the stream's end, the engine ranks the graph's
  other entries after it, exactly, by the vectors the lake holds,
  holding the keys the stream handed out so as to know them. A pull that
  comes back short is the field's end. On the field of 100 000 at
  `ef_search` 1 000, a stream read to its end handed out 59 768 to
  84 870 of its 100 000 vectors (median 75 317) over 30 queries, each in
  1.5 to 1.7 s. On the cost check's corpus of 4 120, a filter keeping
  some 900 records paged to its end was answered by the spike's stream,
  which had no rest, 44 787 of the 45 000 hits the exact ranking gives
  it over 50 queries; the 213 it left out are what the rest holds. The
  build appends the rest, and on the field of 100 000 none of 200 first
  pages of such a filter came back short.
- **Equal scores.** Every member of a tie at the `k`th row that the
  stream hands out is within the margin, so it is read before the pull
  is cut and the keys order it; no deeper pull is asked. A member the
  stream does not hand out is in the rest. So under an index the graph
  decides on which side of the stream's end a member of a tie falls,
  and the score rises where the rest begins: the sequence is best first
  within the stream and within the rest.

### A class of reader is what its grants let it see of a field

A stream is a function of the graph it is read from, and every entry of
the graph decides where its passes go and what they leave out, whether
the reader may see that entry or not. So a reader is streamed only from
a graph that holds nothing it may not see. A reader for whom no such
graph is kept is ranked exactly, where the order of what it may see
follows the cosine alone. Of an indexed field every exact ranking
leaves out a vector of zeros, as the rest after a stream does (025), so
which way a reader was ranked does not show by one.

What decides what a reader may see of an indexed field, and whether it
splits the field's graph:

| what decides it | a graph of its own | why |
|---|---|---|
| `restricted` | yes: two classes | A hold is a fact of the record, the same for every reader, and whether a reader is shown held records is one grant, lake-wide (`Sight::sees_restricted`). Two graphs serve every reader. |
| `only` (051) | none: the exact ranking of what it admits | Its pairs are values each principal carries into its call, so its classes are as many as the principals, and no lake could keep a graph for each. The engine ranks the records that hold the pairs exactly, found as `find` finds them: by the exact text index where the field is looked up (057), else by a scan of the type. |
| a link's ends | none for a reader they judge: the exact ranking | Which links a reader may see follows from the holds of their ends and its `only` on the ends' types, which change with no write of the link. |
| the type's grant | no | A type is read whole or refused before the search, and each type keeps graphs of its own. |
| field grants, the vector field's among them | no | A field is shown on every record of a type its reader reads, or on none. A vector field not shown answers as one the type does not declare (025). |
| `valid_at` and `where` | no | They are the reader's own selection, among records it may see: a record its `where` leaves out it reads by another `where`. A `where` over a field it may not read is refused (024). |
| the position read | no | The hold that stands at the head governs every position (006), and a search at an earlier position ranks by the head (025), so the head's graph of a reader's class holds nothing it may not see at any position. A record written since is one it sees at the head. |

So an indexed vector field of a record type has at most two graphs:

- **graph B** holds the live heads that are not held. Every indexed
  field keeps it, from its first build. It is read for a reader with no
  `only` on the type: one without `restricted` always, and one with it
  while the type has no row for A.
- **graph A** holds every live head. A type gets it once one of its
  heads is held: the fold of the first hold while the type has no row
  for A writes that row, A not yet built, in its transaction, and a
  build that finds a held head builds A with B; a build that finds none
  keeps no A. A is read for a reader that holds `restricted` and no
  `only` on the type; while A's row says it is not yet built, that
  reader is ranked exactly.

B is the graph every field keeps, and A the one a hold adds, so that a
hold changes nothing a reader without `restricted` is answered unless B
may hold an entry of the record. A record written held, the type's
first among them, never enters B, and its write is one that reader
cannot tell. A reader with `restricted` sees the hold, so its move from
B to A, through the exact ranking, follows what it may see. What a hold
of a record B held still tells B's readers is under "What a hold still
tells a reader without `restricted`".

An indexed vector field of a link type has one graph, of every live
head, kept from the field's first build. It is read for a reader that
holds no `only` on the link type and that its sight does not judge by
the ends (`Sight::judges_ends`): one with `restricted` and no `only` on
the ends' types, nor on the ends' types of an end that is a link (006,
051). Every other reader of a link type is ranked exactly, and the store
judges the ends, as today. A graph of links for a reader without
`restricted` would hold the links whose ends are not held, and it would
change at every hold of an end.

So the count is at most two graphs for each indexed vector field of a
record type and one for each of a link type: it follows from the
declarations, `vector_index` and whether a head of the type was held
at the last build or since, never from the grants or from how many
principals there are.

### A class's graph holds only what its class may see

- **One entry per live head.** A graph holds the unit vector of each
  live head of its class that has one in the field, by the record's key.
  The field's history stays apart and unindexed: the projection keeps
  every position's elements once, as 072 keeps them, and only the graphs
  hold unit vectors.
- **The fold writes the graphs at the end of its transaction.** It
  writes each record's entries from its state at the end of what the
  transaction folds, however many batches and pages of states that is
  (038): one in each graph whose class sees it, none in another. A
  record written and held in one transaction is never put into B.
- **An entry not yet committed is of a record its class sees at the
  next position.** pgvector puts an entry into the graph at its insert,
  before the commit, and leaves it there after an abort until `VACUUM`.
  A fold runs after its batch is in the log and synced (061), and a fold
  that fails is folded again from the log, before the writer's next
  write (016) and at its open. So the entries an open write has put into
  a graph are records its class sees at the position the fold reaches,
  and the open write moves a reader's stream as its commit will.
  Measured under "Why": an open write of 1 000 vectors near the queries,
  in every graph. A failed fold may have put into B a record whose write
  and hold the catch-up then folds in one transaction, and what it
  reached cannot be read back, so the catch-up marks B stale (below) at
  every hold it folds.
- **A hold builds B again.** Once a record is held, every entry it ever
  had in B, the live one and the deleted ones of its earlier vectors, is
  of a vector B's class may read at no position (006). A deleted entry
  stays in the graph until `VACUUM`, and `VACUUM` mends the graph's
  links around it, so the graph keeps the shape the entry gave it. So
  a key that had an entry in a generation of B since its copy keeps its
  row in the generation's table, without a vector, once the entry goes.
  The fold, at the hold of a record whose key has a row in B's
  generation, an entry or one gone, deletes the entry if there is one
  and marks B stale from the hold, in that transaction. A hold of any
  other record finds no row of it in B's generation and leaves B as it
  is. No search reads a stale B, at whatever position it reads: its
  readers are ranked exactly, judged by the engine, until a B that never
  held the record replaces it.
- **The writer builds that B beside the old one,** on a connection of
  its own that it holds only while a graph waits, as a table of a new
  generation, which it names in the graph's row before it begins. From
  then on each fold names the keys it touched in a table of its own. The
  writer copies the live heads not held in one snapshot and builds the
  index. Then, in one short transaction that holds the lock the fold
  takes, it writes the entries of every key named since into the new
  generation, a key gone keeping its row; if one of those keys is held
  and has a row there, or B was marked stale meanwhile, which gives up
  the build, it drops the new generation and starts a new copy.
  Otherwise it points the graph's row at the new generation and clears
  the mark. A search reads the graph's row and the generation it names
  in the one transaction that ranks it, under one snapshot, so one that
  began before the swap reads the old generation whole. A search locks
  the graphs' rows before it takes its snapshot, so the writer drops the
  old generation once every transaction that held that lock when it
  looked after the swap has ended, looking again every 200 ms and
  taking no lock a search waits on: searches that overlap without a
  pause delay the drop by the longest of them, never further. At its
  open the writer drops every table of a graph but the one its row
  reads, once no transaction holds that lock, and starts again wherever
  the mark says B is stale. Where holds come faster than a build, B
  stays stale and its readers keep the exact ranking. A is built in the
  same way, once its row is written; a held key in A is no taint. A
  build that fails is tried again after a wait that doubles with each
  failure in a row, from one second up to 64, the writer's connection
  for it closed meanwhile; a commit ends the wait. A drop that waits on
  searches is no failure.
- **Lifting a hold puts the entry into B.** From then on its class sees
  the record, at every position.
- **What a graph keeps of a deleted entry is its own class's.** An entry
  is deleted when its record's vector changes or is cleared, and when
  the record is retired or re-keyed. Each leaves the entry of a vector
  its class may still read `as_of` an earlier position. A hold is the
  one change after which the class may read none of its record's
  vectors, and it is the one that builds B again.
- **An act that erases builds the graphs anew.** A forgotten source's
  values (058) and values taken back (065) are restated by a rebuild
  wherever the type keeps a vector index, so the graphs it builds never
  held them.
- **The setting builds the graphs as it builds today's index.** The
  setting's write builds the field's B, and A where a head of the type
  is held, in the fold's transaction; a redeclaration of the type builds
  them again; `rebuild` and `clear` take them with the projection.
- **A grant changes no graph.** A reader's class is read from its grants
  at each call. A principal given `restricted` is streamed from A from
  its next call, one that loses it from B, and no entry moves.

### PostgreSQL keeps a table per graph

Each graph is a table of the projection: the record's key (`TEXT COLLATE
"C"`, its primary key) and the unit vector (`vector(n)`), with an HNSW
index over the unit vector (`m` 16, `ef_construction` 64, as today) and
no predicate. Its name is drawn from a digest of the type, the field and
the class and the generation, since a PostgreSQL name stops at 63
bytes. A row per graph names its type, field and class, the generation
read and the one being built, and the position B is stale from; it
takes the place of the mark `vindex.<type>.<field>`. A row of a
generation's table without a unit vector is a key that had an entry
since the copy: the stream reads past it.
`vectors` keeps every position's elements and no longer a unit vector,
and the partial index per field goes. Where a host places a lake's parts
in tablespaces (lakelands#1372), the graphs go where its vectors go.

A partial index per class over `vectors` was the other shape, and was
not taken. A hold would write a column on every vector row of its
record, and the deleted entry would stay in that index's graph as in
any other, so B would still have to be built again. Built over the same
table, it would take in the rows a running snapshot may still see, the
held record's last row among them; a copy taken in a snapshot after the
hold does not hold that row. And a table of its own is replaced whole
by the next generation.

What it costs, on the field of 100 000 × 32 with a quarter held, at
PostgreSQL 16 and pgvector 0.8.6, `maintenance_work_mem` 64 MB, built
without parallel workers by hand from a copy of the field's vectors (the
cost runs under "Why" built A in 37.7 and 34.5 s and B in 15.0 and
18.7 s, under more load):

| | today | this design |
|---|---|---|
| build | one partial index, 22.9 s | A 21.6 s; B (75 000) copied in 0.57 s and built in 10.6 s; while nothing of the type is held, B alone, of every live head, as A |
| disk | index 41 MB; the units in `vectors` | A 41 MB index and 20 MB table and key; B 31 MB and 15 MB; while nothing of the type is held, B alone, as A |
| a written vector | one entry, 0.77 to 0.90 ms | one entry per graph whose class sees it, 0.70 to 0.94 ms each: two for a record not held where A is kept |
| a vector's history row | with its unit | without it, 4.4 ms per 1 000 |
| an ended vector | its row's end, 8.3 ms per 1 000 | its row's end, and its entry deleted from each graph, 2.7 ms per 1 000 a graph |
| a hold | nothing | a deleted entry and the mark; B built again beside, 11.2 s for 75 000 entries, while its readers are ranked exactly (84.08 ms a first page, under "Why"); at the type's first hold A too, 21.6 s |
| a grant changed | nothing | nothing |
| the upgrade | — | a rebuild, under "The layout is 7" |

The planner reads each graph by its index, ordered by the distance, and
the head beside it by `vectors`' head index. The stream's statement
turns `enable_seqscan` and `enable_sort` off, so that a small graph is
read by its index too, and turns them on again after it.
pgvector 0.8 stays the minimum: the graphs ask nothing of it the stream
does not.

### PostgreSQL reads one stream at a fixed depth, to the graph's end

The engine opens a cursor over the graph of its reader's class, in the
index's order (`unit <-> query`), and fetches it in growing batches. The
search sets the following with `SET LOCAL`, as it sets the tuple bound
today, so its answer does not follow the server's values:

- **`hnsw.iterative_scan = strict_order`.** No row comes nearer than the
  one before, which the cut needs. `relaxed_order` hands a late vector
  out of order (90 rows in the first 50 574 of one stream), so a pull
  could not stop at its `k`th.
- **`hnsw.ef_search = 1 000`, pgvector's maximum, for every search.** At
  one `ef_search` the stream is the same however much of it a statement
  fetches: the first pass searches `ef_search` wide, and each later pass
  goes on from what the one before discarded. Today the value is
  `clamp(depth, 200, 1 000)`, so it follows the page. At 200 a strict
  stream misses the floor of
  `025 § An indexed field's set is stable between writes, and its recall is held at a floor`
  at k = 100 under a filter: 0.974 on the corpus, against 0.98. At
  1 000 it holds 1.00 there. It cost little in one run of the stream
  under the seam on the field of 100 000: a first page of 6.29 ms
  against 4.46 at 200 with no filter, 5.72 against 5.21 hiding a
  quarter, and 60.99 against 100.61 for a filter keeping some 900, where
  the wider pass hands out more of what its reader may see.
- **`hnsw.max_scan_tuples = 2 147 483 647`, `hnsw.scan_mem_multiplier =
  1 000` and `work_mem = 4MB`.** pgvector ends an iterative scan after
  that many index tuples, or past the multiplier times `work_mem`.
  Today's bound is 20 000 tuples. With the bounds lifted, the stream
  ends where the graph's reach ends. `work_mem` is set as well, since
  `docs/hosting.md` tells hosts to raise it and the bound would follow.
- **The memory bound stays a bound: 4 000 MiB a search.** At the
  multiplier of 1, a strict stream over the field of 100 000 ended after
  27 532 vectors (29 140 at `ef_search` 1 000), some 7 000 vectors
  handed out per MiB at 32 dimensions. 4 000 MiB is then some 28 million
  at 32 dimensions, and fewer at more dimensions (not measured). A field
  large enough to reach the bound ends its stream there, over entries
  its reader's class may see. The bound is a search's: searches running
  at once may each take it, as each parallel worker of one may.
  `docs/hosting.md` says both.
- **`jit = off`.** The statement reads each vector's head record beside
  it. On a field of 100 000 its estimated cost passes `jit_above_cost`,
  and PostgreSQL then compiles it, 190 to 440 ms a search, before the
  first row.
- **pgvector 0.8 or later, as today.** Iterative scans, both orders and
  both bounds came with 0.8.0. `docs/hosting.md` says 0.8 or later, CI
  installs the `postgresql-16-pgvector` package into `postgres:16`
  without a pin, and the spike ran on 0.8.6.
- **The selection is judged in Rust.** A `where` in the statement would
  make the scan read on until enough rows pass: the same stream with
  rows dropped. The engine judges each row by `engines::admits` (031).

### A narrow reader of a large field is answered by the stream

Today a reader whose filter keeps few records of a large field reaches
the tuple bound, its pull comes back short, and the exact scan answers
it: right, slow (128.53 ms a first page and 597.70 a tenth on the field
of 100 000, under "Why"), and at a place that follows from how many of
the nearest 20 000 it may not see.
Under this design the stream answers it, and the stream's drops are its
recall. On the field of 100 000, a reader that sees 900 of it, over 21
queries, against the exact ranking of what it may see:

| stream | first 10 | first 100 |
|---|---|---|
| strict, `ef_search` 200 | 0.962 | 0.915 |
| strict, `ef_search` 1 000 | 0.981 | 0.924 |
| relaxed, 200 or 1 000 (not one sequence) | 0.981 | 0.964 |

The suite's floor holds on the corpus. On a large field a narrow
reader's recall at k = 100 falls below it. Answering such a reader by
the exact scan would need a choice that follows from what it may see,
not from the stream's end. That choice is not proposed here: this
design accepts the narrow reader's recall above, and `docs/hosting.md`
states it. A reader holding `only` is not this reader: it is ranked
exactly.

A pull of such a reader that reaches the stream's end reads the stream
whole, 1.5 to 1.7 s on the field of 100 000, and then the rest. The
spike built no rest, so its tables under "Why" show such a page short
(12 to 16 of 200 first pages); the build's table shows its time.

### What a reader's answer follows

A page's hits, their scores, its `more`, and where its reader's part of
the sequence passes into the rest are all cut from the sequence of its
class. How deep the store pulls follows the candidates hidden from its
reader, and decides only how long the page takes. So none of the counts
the engine and the store make today over candidates a reader may not
see decides an answer any more: the tuple bound and the exact scan
behind it, the depth that follows the page, and the deeper pull for a
tie. The fused lists keep counting their `max_limit` among the hits the
read may see
(`025 § Several rankings are fused by weighted reciprocal rank, among the hits the read may see`).

And the graph a reader is streamed from holds nothing it may not see.
What its stream still follows is its class's own:

- the entries of records its class sees, its own hidden by its `where`
  among them;
- the entries its class's writes deleted, each a vector it may read at
  an earlier position, until `VACUUM`;
- the entries of an open write of its class, each a fact of the log at
  the next position;
- the memory bound, over its class's entries, on fields that reach it.

A reader ranked exactly follows the cosine alone. What a reader may not
see still costs it time: the exact ranking, a stale B, and reading past
what its `where` hides.

### What a hold still tells a reader without `restricted`

A hold of a record that had a vector in B's generation moves B's
readers to the exact ranking, and then to a new generation. The exact
ranking finds what a stream misses, and two builds of one set are two
graphs, so the new generation hands out another stream throughout. So a
reader without `restricted` can tell from its answers that B was built
again: that a record of the type with a vector in the field, one its
class could see at some position after B's last build, has been held,
and about when. It is told nothing of the record, not its key, values
or vector. A write of a record it sees changes B by an entry, and
`VACUUM` mends B's links, but neither changes the stream throughout.

That is what this design leaves, and the only way found to take it away
is to keep no B: every reader without `restricted` ranked exactly, on
the field of 100 000 × 32 at 84.08 ms a first page and 96.40 a tenth
against 6.58 and 10.93 read from B (under "Why"), growing with the
field. A type then keeps one graph, of every live head, which no hold
builds again; today's index over the history already holds only what a
reader with `restricted` may see, so that way needs no layout change.
This record keeps B, as an index per class of reader asks, and accepts
what it leaves: a reader without `restricted` may tell that a record it
once saw was held, and roughly when, never what. `docs/hosting.md` says
so to a host.

### The pages of one query are one sequence while the graph and the reader's class do not change

The stream is a function of the graph, the query and the settings
above. A graph changes at a write of its class, committed or not, at a
build, at the swap of a B built again, and at `VACUUM`, autovacuum
included. Between two pages, or between two batches of one cursor, a
changed graph hands out another stream, and a hit may then come twice
or be left out. A cursor carries the position its first page answered.
A committed write moves the head and not that position, and a page
continued after it ranks by the head's graph
(`025 § A search at an earlier position ranks by the head`), so it may
read another stream; an open write, a swap or a `VACUUM` changes the
graph and moves no position, and the caller is not told. A grant
change between two pages moves its reader to another class, and so to
another sequence. A write its class does not see changes no graph its
reader reads.

So a field paged while nothing its reader's class sees is written keeps
the promise whole. One paged while it is written keeps it as an
unindexed field keeps its order across a write, which is not at all.
Not proposed: a keyset cursor that carries the last hit's place (in the
stream or in the rest, its score and key) and answers what ranks after
it. Across a change of the graph it would keep "none twice" for every
record the change did not move, and still leave out one it moved in
front. It is a change of the cursor alone and can come later.

### The engine judges what a candidate's own state decides, and the store keeps the rest

Today and under the stream alone, the store reads every candidate's
record to judge it. A page of ten whose reader may see three in four
candidates reads some forty records; one whose filter keeps one in a
hundred reads thousands. In one run of the stream alone, a filter
keeping some 900 of the field of 100 000 cost 261 ms for a first page
and 1 379 for a tenth (90th percentile 951 and 2 038), against today's
152 and 739 in that run.
With the seam below, the engine judges each candidate on its head
record, read beside the vector in the same statement, and counts the
pull in candidates its reader may see; the store reads only the page's
records.

The engine judges by the selection the store already hands its `scan`
for `find`, through the shared `engines::admits` (031): retired,
`restricted`, the reader's `only` pairs held exactly, `valid_at`, and a
`where` over the fields the projection holds. A reader with `only` pairs
is ranked over the records that hold them, exactly. The engine reads
the head record in the statement that ranks the candidate, so its
candidates and its judgement are of the one position it answers
(`025 § A search's candidates and its judgement are of one state`). The
store judges the page's hits again at that position. A hit the engine
admits and the store does not leaves the page short, and the store
pulls deeper, as for any hidden candidate. A search the store makes
again at the position answered asks the engine again where the reader's
selection differs there, as it does where the plan does.

The store keeps what it decides before the search: the type's grant and
the vector field's (a field the reader is not shown answers as one the
type does not declare), and a `where` over a field the reader may not
read, which it refuses (024). It keeps what it decides on the page's
hits afterwards: the field masks and `with_meta`. And it keeps the whole
judgement wherever that needs more than a candidate's own state at the
head:

- a reader whose `only` reaches a link's ends, which are other records;
- a reader whose `only` names a field computed at the read;
- a type its reader's settings are narrowed on;
- a `where` over a field computed at the read, or one only the store
  evaluates;
- a search at a position earlier than the head, as a page continued
  after a write is.

There the store judges every candidate, at today's cost. Both paths cut
from the one sequence, so a page from either is a page of the same
ranking.

**The engine is told the class on every path.** On the store's paths
too, the store passes its reader's selection, as far as it judges a
record alike at every position: retired records left out, and
`restricted` as its reader holds it, which the hold at the head
decides. A reader without `restricted` passed nothing would be streamed
from A. Where its reader's records are no class, a reader with `only`
the engine does not judge or one judged by a link's ends, the store asks
for the exact ranking. A reader with `only` the engine judges is ranked
over its pairs.

### SQLite and memory rank exactly, through the same seam

They keep no index. Their sequence is the exact ranking, of which every
pull already is a prefix, and which follows only what its reader may
see, so neither the stream nor the classes change anything there
(measured: no hit twice and none left out, on both fields). With the
seam they score every vector of the field and judge the candidates best
first, a batch at a time, reading a record only where its score could
be among the best `k`. The store reads the page's records.

### The suite holds one sequence for every reader

Over the corpus of
`025 § An indexed field's set is stable between writes, and its recall is held at a floor`,
on every engine:

- A search paged by ten to its end answers each hit once, and it
  answers every hit that one page of 1 000 holds. This holds with no
  filter, with a filter hiding a quarter, with a filter keeping some
  900 records, for a reader without `restricted`, and for a reader
  holding `only`.
- A page of `k` is the first `k` hits of a page of 1 000.
- Without an index, and on SQLite and memory, today's twin keeps
  holding: a lake that holds records its reader may not see answers
  that reader as a lake that never held them does.
- Under an index that twin cannot hold, even with a graph per class:
  pgvector draws each entry's layer at random, so two builds of one set
  are two graphs. Two builds of the same 20 000 entries handed out
  streams that parted between their 18th and their 3 402nd row, over 10
  queries, and differed in length in 8. Its twin there is a write its
  reader may not see. A reader without `restricted`, paged to its end,
  is answered the same hits in the same order before and after held
  records are written, the type's first hold among them, their vectors
  changed and the records retired,
  and a reader holding `only` before and after records outside its pairs
  are. After a hold, until B is built again, a reader without
  `restricted` is answered the exact ranking of what it may see.
- The recall floor stays, at the head of the sequence.

PostgreSQL's own tests hold the rest: a B built again never held the
held record's row, nor a vector removed during its copy; a hold of a
record whose vector was removed since B's copy builds B again; a
half-built B is dropped at the writer's open; a hold folded during a
copy starts a new one; and a projection layout 6 kept under an index is
rebuilt at the writer's open, its owner's and reader's pages as they
were.

### A search under an index is not windowed

The sequence runs through the whole field, the stream and then the rest,
so a search under an index pages to the field's end as an exact one
does. 025's index set is the stream's: the graph decides which vectors
come in the stream and which after it, and the cosine decides how they
rank within each. A fused search keeps its lists of `max_limit` hits.
Each list is now a prefix of one sequence, so the fused ranking is one
for every page as well, and under the seam the engine counts each
list's `max_limit` among what its reader may see. On the field of
100 000 × 32 the build answered a fused search of two fields in a median
36.59 / 34.13 ms for the owner and 40.16 / 48.32 ms for a reader without
`restricted` (first / tenth page), against 15.35 / 54.43 and 70.90 /
102.43 today.

### The seam: `Rankings` says whom the lists are for, `Ranked` says they were judged

```rust
pub struct Rankings {
    // `text`, `vectors` and `text_weights` as they are.
    /// The reader's selection. The engine streams an indexed field only
    /// from the graph of the class `seen.restricted` names, ranks exactly
    /// the records that hold `seen.equal`, and admits each vector
    /// candidate by `seen` on its head record: a vector list then holds
    /// the first `k` candidates of the field's sequence that it admits.
    /// Only its selection is read, not its order, `after` or `limit`.
    /// `None`: every live candidate, ranked as for a reader holding
    /// `restricted`.
    pub seen: Option<Scan>,
    /// Rank every vector list exactly, from no graph: for a reader whose
    /// records are no class.
    pub exact: bool,
}

pub struct Ranked {
    // `position` and `lists` as they are.
    /// Whether the vector lists were judged by `seen`. An engine that
    /// answers `false` is judged by the store, at today's cost.
    pub judged: bool,
}
```

`LakeEngine::ranked(&self, ty, asked: &Rankings, k: u32)` keeps its
signature, and the seam's width stays 89 of 110. The three fields are
additions under 056, which the `host-surface` job lets through
(`constructible_struct_adds_field`). A host that builds either struct
by a literal without `..Default::default()` must add them, and the
release's "Changes a host must make" says so. `judged` keeps an engine
outside the workspace correct when it does not read `seen`: otherwise
its unjudged lists would be taken for judged ones. It does not catch
an engine that judges by `seen` and streams every reader from one graph:
there the promise in `docs/seams.md` below is the only guard, and the
suite's twin under an index holds it for the engines in the workspace.
The other shape is a new method `ranked_seen` that a trait default
answers with "not judged". It leaves both structs as they are, for a
width of 90.

`docs/seams.md` §`ranked` says what an engine that keeps an index
promises: an indexed list is a prefix of the field's one sequence for
the class it is asked for, and its graph holds nothing that class may
not see; an engine that cannot keep that ranks exactly.

### The layout is 7

PostgreSQL's projection gains the graph tables and their rows, and
`vectors` loses its unit vector and its partial indexes. A projection of
layout 6 would answer from indexes that no longer exist, so the engine's
layout becomes 7, and the first open of 0.19.0 rebuilds each PostgreSQL
lake from its log
(`004 § The projection keeps an index of the log, and the log answers`),
beside the projection it replaces
(`071 § The projection is never emptied for a rebuild`). The rebuild
builds the graphs at its end: B, and A where a head of the type is
held. Measured on the field of 100 000 with a quarter held, without
parallel workers: the rebuild today takes 78.0 s, and its partial
index, built alone, 22.9 s; the two graphs take 32.8 s in its place
(52.7 and 53.2 s in the cost runs under "Why"), so the rebuild some 88
to 110 s. SQLite and memory keep their layouts.

A lake converted at the first open is something a host must act on, so
by 068 the layout change is a minor release, and the stream and the
judgement go with it. It touched these records, rewritten in place at
its acceptance:

- `006`, where an engine's vector candidates include restricted records:
  under an index they are its reader's class's, and a held record is no
  candidate of a reader without `restricted`.
- `038 § The layout is 7`, which was "The layout is 6", names the graph
  tables and their rows, and `vectors` without a unit vector.
- `072` keeps a vector's elements once in `vectors`; the unit vectors
  moved to the graphs, and its "The layout is raised" names 7 too.
- `071` names the second thing built beside: a graph's next
  generation, B built again or A at the type's first hold, by the
  writer, with its own crash rule.
- `040 § What it is not`, "Not a promise for a pgvector index": the swap
  of a B built again moves a set too, as `VACUUM` does.
- `029`, `043` and `046` point at 025's headings, and their rows follow
  the headings this record replaces.
- `docs/hosting.md`: the first open's rebuild and its time, the memory
  bound of each search running at once, the graphs' disk, and the exact
  ranking while a graph is built again.

### One fix, in 0.19.0

The three parts ship together in 0.19.0, built from `develop`; 0.18 gets
no patch for it. Its notes name:

- the pages of one query become one sequence, so answers below the
  first page change;
- each reader is streamed from the graph of its class, and a reader
  holding `only`, or judged by a link's ends, is ranked exactly;
- after a hold, a reader without `restricted` is ranked exactly until B
  is built again, and from a type's first hold a reader with it until A
  is built;
- a narrow reader of a large field is answered by the stream at the
  recall above, not by the exact scan;
- a tie can fall on both sides of the stream's end;
- every search reads at `ef_search` 1 000, and its scan may take up to
  4 000 MiB;
- the layout is 7, and the first open rebuilds each PostgreSQL lake;
- the three fields of the seam, under "Changes a host must make".

### What the graphs make unnecessary

- **The judgement of a hold on a graph's candidates.** A reader streamed
  from B is handed no held record. The engine still judges `restricted`
  where it ranks exactly.
- **The unit vector in every row of `vectors`,** and the partial index
  per field over the history, with its mark.
- **What the stream already took:** the tie pulled deeper, the tuple
  bound, the depth's clamp, and the exact scan behind a short pull.

Nothing of the stream or the judgement goes: the stream is what a graph
is read by, and the judgement is what an exact ranking and a `where`
need.

### What it changed in 025

025 was rewritten in place at this record's acceptance. The rules it
held, by their headings then, and what each became:

- `A vector ranking is the cosine, best first, exact to the bit`: under
  an index, best first within the stream and within the rest.
- `A restricted record is a candidate, and the store applies the grants to the hits`:
  under an index a held record is no candidate of a reader without
  `restricted`, a reader holding `only` is ranked over its pairs, and
  under the seam the engine applies the grants, by the shared functions.
- `PostgreSQL keeps the index, and uses it only while it describes the head`:
  a graph per class, kept in tables of their own; the settings above
  replace the depth and its clamp; the graph decides on which side of
  the stream's end a tie member falls; and the sentence left to
  lakelands#1254 is answered.
- `Under an index a tie at the boundary is pulled deeper`: replaced by
  the margin.
- `Under an index a pull that comes back short is answered by the exact scan`:
  replaced by the rest after the stream.
- `Under an index a page is the index's answer at the depth that page asks for`:
  replaced by the one sequence of a class.
- `An indexed field's set is stable between writes, and its recall is held at a floor`:
  the set is the stream of the reader's class, a write of that class
  moves it, an open one at one position, and `ef_search` is fixed; the
  floor stays.
- `A search at an earlier position ranks by the head`: by the head's
  graph of its reader's class.
- `Equal scores of one ranking follow the keys at the head, and a fused tie the key answered`:
  the indexed exception becomes one sequence while the graph does not
  change.
- `What it is not`, "Not one sequence across pages": rewritten to match.
- `Engines find candidates, and the shared functions decide which of them answer, in what order, with what score`:
  under the seam the engine applies them.
- `A field the lake asks an index for answers the index's set, and says so`:
  the index's set is the sequence of the reader's class, and a reader
  ranked exactly is answered the reference, which still says `index`.
- `A search's candidates and its judgement are of one state`: under the
  seam the engine judges each candidate on the head record it reads in
  the statement that ranks it, and the store judges the page's hits
  again at that position.
- `Several rankings are fused by weighted reciprocal rank, among the hits the read may see`:
  each list is a prefix of one sequence, counted by the engine under the
  seam.

This record's pointers to the headings it replaced, and those in 029
and 043, were rewritten with it. In
`docs/decisions/067-a-search-may-take-a-second-stage.md`, "It does not
change a vector index's paging" points here: under an index the second
stage takes the first `top` hits of its reader's one sequence.

### Its size, against the round's reserve

The round of 0.19.0 has a reserve: the product's ceiling at 85 000 and
the specification's at 157 000, from 82 989 and 154 465 on `develop`
(`docs/budgets.md`, lakelands#1325), shared with backup and restore,
placement, metrics and the lease-stale flag, and no row inside it until
it is spent. Its open pull requests measure, on their heads against
their bases, +136 and +235 (backup and restore), +76 and +201
(placement) and −107 and +6 (rights 4/4), so some 1 900 product lines
and 2 090 of specification are left before metrics and the lease-stale
flag. Reckoned from what shipped, not from the spike: the vector stored
once (072) added 766 product lines and 255 of specification for one
layout change of these tables, and the spike's stream and judgement 410
product lines without the rest, the memory engine or their errors.

| part | product | specification |
|---|---|---|
| the stream, with the rest after it | +350 to +450 | +150 to +250 |
| the engine's judgement and the seam, on three engines | +400 to +550 | +200 to +300 |
| the graphs: tables, fold, B built again, layout 7 | +900 to +1 300 | +400 to +650 |
| cut: tie, tuple bound, clamp, units, partial index | −250 to −300 | −50 |
| **total** | **+1 350 to +2 050** | **+700 to +1 150** |

So the fix takes most of what is left of the product's reserve, and
metrics and the lease-stale flag may not fit after it. The build cuts
first and names each pull request's measured delta. Where the round
passes the reserve, the reserve grows once, measured, by the operator's
decision under `docs/budgets.md`, as rounds before it have; the fix
does not shrink to fit it.

Measured on the build, from 83 349 and 155 332 on `develop` at its base
(metrics and the lease-stale flag merged): product 84 498 (+1 149) and
specification 156 323 (+991), inside the ceilings, below the
reckoning's lower end; no row is added.

## Why

Two pages of one search under an index are cut from two answers of the
index. The store asks deeper for a later page, pgvector searches wider
at a deeper `ef_search`, and a neighbour the first page's read missed
lands behind the later page's offset. Measured below through the store:
up to 57 hits answered twice, and 57 records never answered, in 50
queries paged to their end. And where a reader's pull runs out of the
20 000 tuples the scan may read follows from the nearest entries it may
not see.

A fixed `ef_search` and a strict order make every pull a prefix of
every deeper one. The store's lazy read, which reads only as deep as
the page needs, is then not the fault but right. With the bounds that
count candidates lifted, no count the engine or the store makes decides
an answer. The engine's judgement then takes away what remains of the
cost: the record the store reads for each hidden candidate.

That leaves the graph. Over one graph of every entry, the stream's first
pass hands out the entries it finds nearest, a reader's or not, so
where it begins to leave out a reader's own vectors follows from what
the reader may not see; an entry is in the graph before its write
commits, and after the write aborts. No order over one graph takes that
away. A graph that holds only what its reader may see does, and the
grants divide readers into few enough classes to keep one each: two,
and for every class beyond them an exact ranking that follows the
cosine alone.

Measured on one development machine with other work beside it: release
build, PostgreSQL 16 with pgvector 0.8.6, and SQLite. Each figure is the
whole call through the store, the median of 200 searches of a page of
ten, first page / tenth page, in ms. "Twice / never" counts hits
answered twice and records never answered, over 50 queries paged by ten
to their end against one page of 1 000. A quarter of the records is
held; the reader holding `only` sees another quarter.

PostgreSQL, the cost check's corpus of 4 120 records (first / tenth
page in ms; twice / never):

| reader | today | the stream and the judgement | this design |
|---|---|---|---|
| no filter | 6.51 / 13.33; 18 / 18 | 3.81 / 7.13; 0 / 0 | 11.19 / 19.74; 0 / 0 |
| a filter hiding a quarter | 5.92 / 14.38; 57 / 57 | 6.82 / 11.39; 0 / 0 | 4.51 / 8.00; 0 / 0 |
| a filter keeping some 900 | 3.37 / 27.75; 6 / 6 | 5.72 / 19.99; 0 / 0 | 4.09 / 15.04; 0 / 0 |
| a reader without `restricted` | 3.71 / 16.67; 23 / 23 | 5.97 / 10.55; 0 / 0 | 6.90 / 9.44; 0 / 0 |
| a reader holding `only` | 18.81 / 69.97; 50 / 50 | 12.82 / 23.92; 0 / 0 | 5.66 / 10.41; 0 / 0 |

The modes of this table ran one after another, over 74 minutes, while
other work on the machine raised its load from 10 to 22; the first row
of this design ran at the highest. The same statement under the open
write below, minutes later, took 3.28 / 6.57. "Never" counts against
the mode's own page of 1 000; against the exact ranking, the stream
leaves out what it does not hand out until the rest is built (213 of
45 000 for the filter keeping some 900, above).

An open write of 1 000 vectors near the queries, not committed, in
every graph and in the partial index, held open while each row ran:

| reader | the stream and the judgement | this design |
|---|---|---|
| no filter | 3.08 / 5.09; 0 / 0 | 3.28 / 6.57; 0 / 0 |
| a filter hiding a quarter | 5.20 / 8.80; 0 / 0 | 5.48 / 9.14; 0 / 0 |
| a filter keeping some 900 | 4.37 / 12.13; 0 / 0 | 6.37 / 18.26; 0 / 0 |
| a reader without `restricted` | 3.51 / 6.59; 0 / 0 | 4.51 / 5.84; 0 / 0 |
| a reader holding `only` | 7.75 / 12.81; 0 / 0 | 5.53 / 8.72; 0 / 0 |

PostgreSQL, the field of 100 000 × 32 (first / tenth page in ms). The
modes ran one after another for each reader, so each row compares under
one load:

| reader | today | the stream and the judgement | this design |
|---|---|---|---|
| no filter | 5.27 / 14.95 | 5.97 / 10.03 | 4.65 / 6.31 |
| a filter hiding a quarter | 4.79 / 9.50 | 5.88 / 7.33 | 5.60 / 9.36 |
| a filter keeping some 900 | 128.53 / 597.70 | 88.53 / 166.68 | 109.29 / 177.65 |
| a reader without `restricted` | 6.69 / 21.60 | 6.26 / 9.64 | 6.58 / 10.93 |
| a reader holding `only` | 382.18 / 920.17 | 109.36 / 113.18 | 67.00 / 70.53 |

At the 90th percentile a first page hiding a quarter took 151.52 ms
today, 45.50 under the stream and 58.79 under this design. The spike
builds no rest after the stream, so a filter keeping some 900 ended 16
and 12 of its 200 first pages short, at 9.3 and 9.5 hits a page, and a
reader holding `only` streamed with the judgement 20 (9.7 hits a page);
ranked exactly over its pairs, it ended none short. The run built the
graphs without parallel workers, A of 100 000 entries in 37.7 s and B of
75 000 in 15.0 s, and a second run in 34.5 and 18.7 s.

After a hold, until B is built again, a reader without `restricted` is
ranked exactly. Each column from its own run:

| field | B read | B stale |
|---|---|---|
| the corpus | 6.90 / 9.44; 0 / 0 | 9.02 / 11.62; 0 / 0 |
| 100 000 × 32 | 6.58 / 10.93 | 84.08 / 96.40 |

The build, through the store on the field of 100 000 × 32 with a
quarter held, each column from its own run on the machine alone, today's
from `develop` before the fix (median first / tenth page in ms; every
first page held its ten hits; the reader holding `only` sees a
quarter):

| reader | today | the build |
|---|---|---|
| owner, no filter | 3.79 / 11.89 | 7.55 / 12.87 |
| owner, a filter hiding a quarter | 5.78 / 28.46 | 11.98 / 20.83 |
| owner, a filter keeping some 900 | 224.83 / 799.90 | 239.32 / 423.30 |
| a reader without `restricted` | 5.74 / 25.93 | 5.59 / 7.96 |
| the same, a filter keeping some 900 | 605.08 / 1 450.65 | 183.75 / 399.17 |
| a reader holding `only` | 532.51 / 1 253.33 | 133.95 / 137.23 |
| fused, two fields, owner | 15.35 / 54.43 | 36.24 / 33.64 |
| fused, a reader without `restricted` | 70.90 / 102.43 | 35.10 / 37.24 |

At the 90th percentile a first page hiding a quarter took 230.75 ms
today and 138.98 in the build. What the graphs cost, in the same runs:

| | today | the build |
|---|---|---|
| a quarter held, 25 000 holds | 123.5 s | 222.9 s, then A and B built in 88.2 s |
| one written vector, median (p90) | 6.16 (10.21) ms | 8.73 (13.63) ms |
| one hold, median (p90) | 4.21 (5.48) ms | 13.11 (15.79) ms, B built again in 32 to 35 s |
| a reader's first page right after a hold | 3.21 ms | 76.99 ms, ranked exactly until B is built |
| the first writer's open after the upgrade | 67.1 s | 89.2 s, the graphs built in it |

## What it is not

- **Not a graph per principal.** `only` is answered exactly, and the
  graphs follow the one grant that divides every reader in two.
- **Not a time promise.** A page's time still grows with what its reader
  may not see where it is ranked exactly or its `where` hides much.
- **Not the exact ranking.** The stream's set is the index's. On the
  field of 100 000, five of the 15 queries between two clusters were
  handed at most 7 of their exact 100 nearest, by the stream and by
  today's scan alike (0, 0, 7, 0 and 1). The rest after the stream holds
  what the index misses, after it, and the suite holds the recall at the
  head of the sequence.
- **Not a window.** A window holds the first `max_limit` hits a reader
  may see, read from a relaxed stream. It also answered one sequence (no
  hit twice, none left out), but it counts `max_limit` visible hits
  before the first page: in one run on the corpus, 30.57 ms for a first
  page hiding a quarter against today's 6.48, and 68.29 against 5.43
  for a filter keeping some 900. It also ends a search at `max_limit`.
- **Not a new setting**, and not a new pgvector minimum.
- **Not a graph per position.** A search at an earlier position ranks by
  the head's graph of its class (025).
- **Not the engine at the head for an earlier position.** Judging a
  continued page at the head and again at the cursor's position would
  leave out a record the reader may see at that position and not at the
  head. So an earlier position is the store's, told only the class.
