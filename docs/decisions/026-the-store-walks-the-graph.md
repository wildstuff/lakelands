# 026 — The store walks the graph; engines answer edges

## What is decided

### A walk is implemented once, in the store's core

Graph walks — one hop, a chain of steps, a subgraph, a shortest path —
are implemented once, in the store's core, over two questions an engine
answers: the links of a record, and whether a record is visible. A
derived relation has no links: over one, a walk reads the records of
the other side that hold exactly the near record's values
(`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
Engines never implement a walk. Every walk orders its own answer
(records by depth, type and key; links by their ends), so the answer
does not depend on the order an engine returns links in.

### A graph includes its start

A graph projection always includes the start node, the root a viewer
draws from, whatever the `min` of its steps. A rows projection holds a
start only where its steps reach a match from it: every row binds its
start beside the records its steps reach, and a start no match begins
at is in no row. Either way the answer keeps the one order: depth, then
type, then key, and says each record's depth: a row's `<name>._depth`,
a graph's `depths` (018).

### The whole-graph measures are walks

Degree centrality and connected components are computed once, in the
store's core, over the same two questions every walk asks, so an engine
answers them without knowing they exist. They count what a walk would
follow: link records and via references live at the read's position,
the matches of a derived relation as that position holds the values
(044), a far end that exists and is not retired and is of a type the
caller may see, a restricted link only for a caller with the grant. A
degree reads the links of a page of records at a time and the start's
records a page at a time, so measuring a whole type holds a page and the
best rows asked for, not the type. Components grow from a page of start
records at a time, the links of a page of the records reached read
together whichever start reached them, and two starts of a page that
reach each other are one component; so the partition costs a question of
the engine per page, relation and hop, not per start. With no start type
the starts are the records of every type shown that declares a step's
relation, read a page at a time, and one with no link it may see is never
counted (018). Components hold every record they reach, up to the lake's
`max_graph_nodes`, and are refused `too_large` past it, never cut.
PageRank and communities hold the same graph, `page_rank` walking each
step's relation in its own direction, and are bounded alike.

**A measure adds integers, never floats** (lakelands#1412, accepted for
0.20.0). The store hands a measure a set of records and a set of links,
each sorted and without repeats, so a different order from an engine
gives it the same input; what could still differ is arithmetic, since a
sum of floats taken in another order may differ in its last bit (031).
So every sum a measure takes is of integers, which is the same in any
order, and its records are indexed by type and key, the store's one
order (025).

- **PageRank** holds the whole mass as `10^12` units in `u64`. Each
  record starts with an equal share, the units left over going one each
  to the first records by index. In a round each record with links out
  sends, along each of them, `floor(85 · mass / (100 · links out))`:
  damping is 85/100, fixed, with one floor a record and none a link.
  What no link carried, the part damping keeps back, the mass of records
  with no link out and every unit a floor dropped, is spread evenly
  over every record, the remainder again by index. So the mass is
  exactly `10^12` after every round and no sum can overflow. There are
  at most 100 rounds, and the first whose scores equal the round
  before's, unit for unit, ends them early: every round after it would
  give the same scores, so the answer is the one 100 rounds give, and a
  graph whose floors never settle runs all 100, still the same on every
  engine. A score is answered as its units divided by `10^12`, one
  correctly rounded division of two exact numbers, so its bits are the
  same on every target, and two different scores never answer one
  value.
- **Communities** move records between groups by the gain in
  modularity, compared as an integer product so that no division is
  taken: moving record `i`, of degree `k_i`, into a group it has
  `k_{i,C}` links into and whose degrees sum to `Σ_C` without it gains
  `2m·k_{i,C} − k_i·Σ_C`, the gain times `2m²`, in `i128`, checked. A
  link from a record to itself counts twice in its degree. A record
  moves only where the gain is strictly more than staying, and only
  into a group it has a link into; a tie
  goes to the group whose least member by type and key is least.
  Records are visited by type and key, sweep after sweep until none
  moves, then the groups are folded into records, linked by the number
  of links between them, and moved again, until a level moves none.
  Every move raises a bounded integer, so the method ends without a cap
  on rounds. A group this leaves internally disconnected is then split
  into its connected parts, as components are found, so every community
  is connected (lakelands#1514).

**A measure's page.** A read keeps nothing between calls, and a page in
the middle of a measure cannot be made from less than the measure: a
component's number and size are known only when the whole partition is.
So a measure is computed whole for every page, and a walk through small
pages computes it once per page (lakelands#474). The way to one
computation is one page: a measure's `limit` may go up to the lake's
`max_graph_nodes`, the bound the lake already puts on a whole-graph
answer, where every other read stops at `max_limit`. A components answer
always fits that page. An absent `limit` stays the lake's default, so an
agent that names none is not handed the whole answer. Small pages are
still answered, at their price; the `query` tool says so.

**A graph's page** (lakelands#1027, lakelands#963). A neighbourhood's
starts are its first records, at depth 0 in key order, and its walk goes
only as deep as its page's last record, so a page reads as many starts as
it reaches and one more. A chain's graph is walked from a page of 256
starts at a time, each step's records together, as rows are. Nothing
shorter than the whole chain tells a page what it holds: a record's depth
is the fewest hops of any match from any start, and a record or a link is
in the graph only where a whole match runs through it. So the chain is
walked whole for every page. What it holds, its starts and every record
each walk reaches whether or not a match keeps it, is counted as it
grows, as a reach's is, and past the lake's `max_graph_nodes` it is
refused `too_large`, never cut. Measured on 10 000 records on PostgreSQL, a
chain's page went from 1.2–2.6 s to 113–169 ms (5 508–9 007 statements
to 57–72), and a neighbourhood's first page of 50 from 8 000 starts from
53 to 4 ms.

**A reach walks a step at a time** (lakelands#991). Each step is walked
whole from every record the step before it kept, the links of a page of
256 of them read together, before the next: "first reached" needs every
earlier step. What it holds is counted as it grows, each source's walk
apart, and refused `too_large` at the lake's `max_graph_nodes`. A step's
`where` reads each record from the read that asked whether it is shown,
none one by one. A page walks until it holds one record more than it
answers and walks no step past that; its cursor names the last record
answered, so a later page walks again the steps up to that record's. A
page costs the steps up to its last, not the whole walk. Measured on
10 000 records (100 regions, 1 000 customers, 5 000 bills, 3 900
payments; three steps, the second with a `where`; pages of 1 000): a
first page, which walks two steps, 76 ms on SQLite and 113 ms on
PostgreSQL; a page in the third step 119 ms and 1 021 ms, most of the
latter the shown-or-not read of the third step's records. Before each
record a `where` read was read alone: 111 and 154 ms, 1 488 and
2 372 ms.

**A page of rows walks a page of starts** (lakelands#940). Rows are walked
from a page of 256 starts at a time, each step's hops asked for the
page together, and a record read once for the walk, a step's `where` and
its row. With no `order_by`, or one whose every path is a stored field of
the start's own, the engine hands the starts in that order (041) and the
rows come in the starts' order, each start's in the walk's (029); the
walk stops at the page and one row more, so `more` is exact: a cursor
follows exactly where a row does. The cursor names the last start whose
rows are all answered, which its reader was answered rows of, and how
many of the next start's rows are, and the next page reads the starts
after that one. Any other order, and an aggregate, needs every row: the
rows are read whole, held as their sort values and records until the
page is cut, and at most the lake's `max_graph_nodes` of them; past it
the read is refused `too_large`, naming `order_by` or `project` and the
way out, never cut, as a measure is. What a reader without `restricted`
or with `only` is answered does not move: the starts are the records a
page answers it, and a far record is kept by the walk as before, so a
page boundary, `more` and a cursor tell nothing of a held start or end.

### The cheapest walk holds the neighbourhood it may pass through

`cheapest` (018) is accepted for 0.20.0 (lakelands#1412). The walk that
costs least within the steps' hops is not the one that costs least
without a bound on hops, which is what a search that settles the nearest
record first finds: a cheaper walk of more hops than the steps allow
would settle a record that the walk within them cannot use. So the store
holds the neighbourhood within the steps' hops, read level by level a
page at a time as a path reads it, and then relaxes it in memory one hop
at a time: after hop `k` each record holds the least walk of exactly `k`
hops that reaches it, compared by cost, then the record it came from by
type and key, then the link by relation and ends. The answer is the
least by cost, then hops, of those that reach `to`, unwound hop by hop.
Holding the neighbourhood is required, not a shortcut, since the
cheapest walk may pass any record within reach; past `max_graph_nodes`
it is refused `too_large`, never cut. Every choice is the least of a set
under one total order, so the order an engine returns links in cannot
move it.

A weight is a decimal at 9 places, held as an integer in `i128`: an
integer enters exactly, a float by its shortest decimal form, a decimal
as written, each rounded half away from zero to 9 places as a money
value is. Costs are summed with checked addition, and a sum past
`i128` is refused `overflow`; no float is added anywhere. Which links
the walk weighs follows from the graph alone, so a link refused for a
missing or negative weight is the same on every engine.

## Why

A walk implemented by each engine is a walk that can differ between
engines, and the difference would show only on the graph that exposes
it. Two primitives, and a read of the matching records for a derived
relation, are enough for every walk the pattern query needs, and
keeping the walks in one place keeps the engine seam narrow and the
answers identical.

## What it is not

- It is not a limit on performance work later. A faster walk is a change
  to the store's implementation, checked against the same answers.
- It does not follow a link to a record that no longer exists or is
  retired; that end is not visible.

## How it came to be

- The first version: walks implemented once, in the store's core, over
  two questions an engine answers.
- A graph projection always includes its start.
- Degree centrality and connected components joined the walks.
- A measure's `limit` may go up to the lake's `max_graph_nodes`, so one
  page holds a measure computed once (lakelands#474).
- Corrected at the rewrite (lakelands#817): a walk over a derived
  relation reads the matching records of the other side, and a measure
  counts its matches, which 044 added and this record's two questions
  and its list of what a measure counts left out; a degree reads the
  links of a page of records at a time, where the record said one
  record's.
- Corrected at the rewrite (lakelands#817): a rows projection was said
  to hold a start "only when a step reaches it", which also reads as a
  row that holds its start only where a step arrives at that record.
  Every row binds its start, whatever the `min` of its steps; a start
  is left out only where no match begins at it.
- An answer says the depth it is ordered by (lakelands#989).
- A page of rows walks a page of starts and stops (lakelands#940): every
  page walked every start, bound every row and sorted them before it
  was cut, about four questions of the engine a start, 19 s for a first
  page of 50 over 10 000 starts on PostgreSQL. Rows read whole for an
  order over a far record or an aggregate gained the bound of a measure.
- Components grow from a page of starts at a time (lakelands#1004): each
  start grew alone, a question of the engine per start, relation and
  hop, each in its own read; 48 000 questions, 45 s on PostgreSQL and
  3 s on SQLite for 4 000 records read in four pages, against 0.18 s on
  memory. The partition is the same.
- With no start type, components grow from the records of every type
  that declares a step's relation (lakelands#992).
- A reach walks a step at a time (lakelands#991).
- PageRank and communities joined the measures, in integers so that no
  sum depends on the order links come in, and the cheapest walk joined
  the walks as a relaxation by hops over the neighbourhood it holds
  (lakelands#1412).
- Communities were built on the graph PageRank holds, its steps walked
  `both`, and split into connected parts with the components' joins
  (lakelands#1514).
