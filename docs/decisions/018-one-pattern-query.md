# 018 — One pattern query, and never a second planner

## What is decided

### Walking relations is one query, and its named constructions are not doors

Walking relations is one query: a start (a type with a key, a filter or
a search), a list of steps (relation, direction, target type, filter,
depth), and a projection (rows for agents, a graph for viewers, or
aggregates). `neighbors`, `traverse`, `path` and `subgraph` are named
constructions of it, not doors of their own. Every answer carries its
position and can be pinned with `as_of` and `valid_at`.

Any later query language a caller writes in (a read-only openCypher or
GQL subset, for example) is translated into this query. There is never a
second planner and never a write path through a query language.

### Too deep is refused, not shortened

A step that names no depth is `1..1`. A step that asks for more hops
than the lake's `max_depth`, or whose `min` is above its `max`, is
refused as invalid. It is never quietly shortened to the limit: an
answer that walked less than it was asked to would look complete when it
is not, and the caller could not tell.

A depth no walk takes is refused before a record is read. The door
judges every step's depth once its steps are judged, so the refusal does
not depend on what the lake holds.

### A step over a relation its type does not declare is refused, and a neighbourhood's step may name a relation any type declares, if its reader is shown it

The planner knows the ontology. A step over a relation its type does not
declare is refused, not answered empty, and grants apply per step,
because every step has a type.

A graph pattern whose steps all start at depth 0 is a neighbourhood:
one walk from its start over any of its steps' relations, each walked
in its own step's direction to its own step's `max`. The construction
`subgraph` builds a neighbourhood: its steps are one walk over any of
their relations, not a chain of steps. Several starts are one walk: each
record is at the fewest hops any start reaches it in. A record reached
over a step's relation is kept, and walked on from, only where that
step's `type` and `where` hold of it. Since a neighbourhood walks into
types its start does not declare, a step may name a relation any type
declares, if its reader is shown the relation. Its `where` is read
against the type that relation reaches, wherever it is declared: its
target walked out, its owner walked in. A step that may reach several
types (walked both ways, or a relation several types declare) and reads
a `where` is refused as invalid unless it names its `type`.

A neighbourhood over a relation no type declares is refused. A graph
whose steps all start at depth 0 is not judged step by step: a step's
relation may be declared by a type another step reaches. A relation no
type its reader is shown declares is walked from nowhere. It is refused
as a chain's step is, naming the type the step is read from (the
start's, for a first step), the relation as the reader sent it and the
walk, with a `type` or without. Only shown relations are read, so a
hidden relation and an absent one are answered alike (006), and an
inverse that is not around the type its step is read from is a relation
of that name: none.

### Any other graph pattern is a chain, walked as its rows are

Any other graph pattern is a chain, walked as its rows are: each step
from the records the step before it kept, with its own relation,
direction, depth range, `type` and `where`, and a step over a relation
its type does not declare is refused. A chain's graph is the start,
every record a match binds, the records each was walked through from
the record before it, and the links walked, so no link has an end
outside the graph. Several starts are one walk here too: each record is
at the fewest hops any start reaches it in. A graph of several steps
that name no depth is a chain, since each is `1..1`; a neighbourhood
names `0..` on each step.

### A graph cut by `limit` pages as rows do

A graph cut by `limit` says `truncated` and carries a `next_cursor`,
exactly as a page of rows does. Each node comes with its depth, in the
order of the nodes (`depths`): the fewest hops that reach it from a
start, 0 for a start, which keeps 0 however a cycle comes back to it (a
graph holds each record once; the edge back says the hop the cycle
closes at). Along a path a node's depth is its hop. The next page is the next `limit`
records of the one order (depth, type, key), with every link between a
record of that page and one of this or an earlier page, so the pages
together are the graph, each record and link once. Each page walks
again, at the cursor's position, as deep as its last record.

### A pattern with `to` is a path, and a path is a graph

A pattern with `to` is a path: the shortest walk to that record, over
its steps' relations as a neighbourhood walks them, each in its step's
direction and only through records its step's `type` and `where` keep.
A path has one start, so a pattern whose start names several records
is refused as invalid, as is one with a step that does not start at
depth 0. A path is a graph, and a pattern with `to` that asks for rows
or an aggregate is refused as invalid: an answer that ignored what it
was asked would look like more than it is. `cheapest` alone is a path
answered as rows (below). Where no path is, the answer is an empty
graph. A path and an aggregate answer in one page and hand
out no cursor, so a cursor handed to either is another read's, and is
refused as such. A path or a neighbourhood with no steps walks no
relation: the neighbourhood is its start alone, and a path reaches no
other record.

A pattern with `to` that asks for rows or an aggregate is told so before
its steps are judged: a path's steps start at depth 0, and rows would
refuse them for that.

`cheapest` is the walk to `to` that costs least, where a link costs its
`weight`: a field of the link type that carries its step's relation
(035), an inverse's being its owner's. It is accepted for 0.20.0
(lakelands#1412). It is a path in every other respect: one start, steps
from depth 0, a relation used at a hop only within its step's depth,
every hop within `max_depth`, and records kept by each step's `type` and
`where`. Least cost wins, then fewest hops, then at each hop the least
record it came from by type and key over the least link (026), so a
cheaper walk of more hops than the steps allow is not taken, and the
walk chosen never passes a record twice. Its rows are the records along
the walk in hop order: `hop` (0 for the start), `type`, `key`, the
`relation` and `walk` of the link that reached it (null at hop 0), and
`cost`, the sum so far. `cost` is a number where every step's weight
field is of kind `number`, and a decimal read as a decimal field reads
otherwise. Where no walk is, it answers no rows; a walk from a record to
itself is that record alone, at cost 0. It answers in one page with no
cursor, as a path does.

Its refusals come in this order, every one before a record is read but
`path_starts` for a filter start and the last: `cheapest` without `to`
is refused `cheapest_needs_to`; `order_by` as a measure refuses it; each
step as a path's is judged; then each step's weight field, in the steps'
order; then several starts `path_starts`, where `path` judges them.
Several starts are known only once a filter start's records are read, so
this one comes after the steps and the weight fields, not before them. A
relation carried by `via` or `derived` holds no link record, so its
weight is refused `weight_not_on_link`, naming the relation and the
field, and so is a field its link type does not declare or hides from
the caller (006). A field the caller is shown and may not read is
refused `not_granted`, as a probe is (024), and a field whose kind is
neither `number` nor `decimal`, through any value type built on them,
`weight_not_number`. Once records are read, a link the walk weighs whose
weight is null or absent is refused `weight_missing` and a negative one
`weight_negative`, each naming the relation and the link's ends; past
`max_graph_nodes` the walk is refused `too_large` and a cost past its
number type `overflow` (026). A missing weight is not skipped and not
read as 0 or 1: a walk that dropped or re-priced a link would look
cheaper or dearer than it is, and a host that wants a weight by default
declares it on the field (050). A negative weight is refused because
over a cycle of negative cost no walk is cheapest; a weight of 0 is a
weight.

### An `order_by` orders rows and nothing else

An `order_by` on a graph, a path or an aggregate is refused. A graph
comes by depth, type and key (025), a path by its hops and an
aggregate's groups by their key, so an order names nothing they honour.
It is refused as invalid where `as` is, before a step is judged. A
measure refuses it in its own words.

Rows are ordered by the values at their paths alone. A row is a
projection of a walk, not a record: a key that asks for `latest`,
`seen_at` or `known_at` is refused, and an ill-formed key as `find`
refuses it, all before a record is read.

### A row binds each record under a name of its own

A row is one match. A match that reaches one type twice holds two
records: a spoke's owner and that owner's boss, or a person and someone
who reports to them. Were each record bound under its type's name, the
second record would overwrite the first, and the row would answer as if
the first had never been reached: ordering, projecting or grouping by
the first record would not be possible.

Each record a pattern binds has a name. The start and each step may
give one with `as`. A record without one is bound under its type's name.
A row holds `<name>.<field>`, `<name>.key`, `<name>._depth` and
`<name>._type`, and every path a pattern names reads those names.
`<name>._depth` is the hops the record's step took to reach it, 0 for
the start: the depth the walk already orders by, which a caller could
otherwise learn only by asking each depth on its own. `<name>._type` is
the record's type. A field is still judged on its record's type: a
field its reader may not know of answers under any name as it does on
the type, and a refusal names the type, not the name.

Under a bound name, `_depth` and `_type` are the lake's own columns,
never fields. A field of either name that a lake already holds is not
reachable from a pattern's rows, and `get` and `find` answer it under
its name. So that the lake's columns never meet a declared field, a
declaration, a set and a migration refuse a new field whose name begins
with `_` (`field_name_kept`). A field the type already holds keeps its
name, as a lake written before the rule holds it, and under a bound name
any such field but `_depth` and `_type` is bound as a field. Were the
lake's columns bare words, a type's field `depth` would hide the
record's depth wherever it is declared, and a name over several types
would read the field for one type and the column for another.

A pattern that would bind two records under one name is refused before
any record is read. A name is never numbered for the caller: an order or
a group key names a path before the answer exists, so the caller must be
able to know every name. A name given by `as` is an identifier. `as`
means nothing to a graph, a path, a degree or components, which answer
nodes or fixed columns, so it is refused there, as `to` is outside a
graph.

### Under one name a field reads alike on every type, so a step over several types reads only what they declare alike

A type the pattern binds is the one type each step reaches, or the
types a step reaches where it names none. Under one name a path reads a
field the same way on every record it binds, so a step over several
types reads, in its `where`, an order, `fields`, a group key or a
measure, only a field each type it reaches and its reader is shown
declares alike: in one storage kind and, where one is named, one value
type, a reference to one target type and a vector of one length.
Anything else is refused `step_several_types`, naming every type the
step reaches, the field, the types that lack it and those that declare
it otherwise than most of them do (than the first type's, by name,
where they split evenly), so the odd one is the one named. A field the
reader may not know of on one of the types is one that type lacks, so
the refusal tells it nothing a typed read would not. Which types a step
reaches is the planner's alone, the types an inverse reaches included;
nothing else reckons them. A `where` that reads through a relation, not
a field, is read against one type and names the step's `type`. An open
name's row is shaped from the types it binds, never from the type a
range's first hop reaches.

A field read over several types is read against each as a typed read
of that type reads it. Its reader masked from it, or allowed to write
it alone, on any of them is refused as that typed read is, the field
named by its type (`<type>.<field>`): no type is left out of the answer
without a word, and the answer never turns on which types happen to
hold a record. `fields` leaves a field masked on one type out of that
type's rows, as it does on a typed read.

The answer is one: one order, one cursor, one measure over every type,
and `<name>._type` names each row's type. A decimal sum adds as decimals
across the types, as over one. Its records are bound under the step's
`as`, or under its relation as the caller wrote it (the inverse's name
where it named the inverse), and a row holds their `key`, `_type`,
`_depth` and the fields the types declare alike. Its key, `_type` and
`_depth` are the lake's columns, read against no type and never probed,
so a field `key` one of the types declares turns nothing. A plain
`count` reads no field, so it counts what the step reaches over every
type, and grouped by `<name>._type` it counts each type: the answer a
caller would otherwise add up from one typed query a type. A step
another follows still reaches one type or names it: the next step is
read from one type. A host names with `as` a step that may reach
several types: left out, its name follows the types its reader is
shown, so a reader shown one of them binds under that type's name, as
on a lake without the others, and one query would not give two readers
the same columns. With its `type` the step binds that type's records
alone, and the step after it is read from that type. Were a step to
bind only the first type that declares its relation, which paths its
rows could be read by would depend on the order of the declarations,
and its `where` would be read against that first type and drop the
other's records without a word.

The `type` of a step of rows, an aggregate or a graph may be any type it
reaches from where it stands, over its whole range in a chain. A measure
takes none. One it does not reach is refused wherever the step reaches
some type, a graph from depth 0 included, and the refusal names every
type it does reach. A refusal at a step names the relation as the caller
wrote it, so a step over an inverse is named by the inverse.

What a step reaches is counted over its whole range: every type a hop of
it arrives at, and the type it stands on where it starts at depth 0. A
relation two types declare reaches both; so does a step whose second hop
leaves the type its first arrived at, and one from depth 0 over a
relation to another type. Rows are walked as a chain whatever their
depths, each step from the records the step before it kept, so the rule
is the same at every step and from depth 0 as in a chain: the step is
judged from where it stands, and a relation that type does not declare
is refused there as well. A chain's graph is judged over the same range,
since it walks what its rows walk: its `type` may name a type a later
hop reaches, and its `where` names one where the range reaches several.
A graph binds no name, but the step after a step is judged from one
type, as its `where`, its `type` and its own relation are read. So a
chain's graph step that another step follows reaches one type or names
its `type` as well: were the next step read from the first of the
types, it would drop the others' records without a word. Its last step
may reach several: no `where` and no later step reads them, and the
graph holds the records of each. At a `valid_at` each of them is kept by
its own valid time, so the step probes the valid-time fields of every
type it can meet (024). A range that reaches no type at all, its walk
ending before its first hop that counts, is judged by its first hop.

### A step's relation is probed before the step is judged

A step's relation is probed around the type the step stands on before
the step is judged, in rows, an aggregate and a graph alike, so a reader
held from the field that carries it there is answered that first. A
later hop, and a relation a neighbourhood meets around another type, is
probed when it is walked.

A step with a `type` reads what it keeps, and a step by an inverse the
one relation that declares it: both are said where the rule is, in
`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md` and
`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`.

### Every path a pattern names is one its rows hold

Every name in a pattern's aggregate is read against the paths the rows
hold. A field key and a measure's field name one whole. Each path in a
named key's expression is read as the longest name the pattern binds,
then one of its fields or `key`, then members by 028's rule for a path.
A field key answers under its path in the group's `by`. A path that no
bound name starts is refused, as a step over an undeclared relation is
and for the same reason: it reads nothing, and an answer built on it
looks like one that read something. The refusal is the same for a type
that does not exist and one its reader is not shown. A named key that
read `bill.amount` as the field `bill` and then its member `amount`
would find nothing and group every row under null, and a name that
named nothing would answer quietly: a field key `zz`, a sum of `zz` as
0.

A named key is held to what `find` holds it to. Each field it reads is
probed before a value is read (024), so a field its reader may not know
of is answered as one the type does not have, as a field key on it is.
An operation it can never compute is refused (028), checked over the
row's shape: one field for each path, of its declared kind.

An order and a projected field read their path by the aggregate's rule,
named whole: the longest name the pattern binds, then one of its fields
or `key`. Any other name is refused as the aggregate refuses it.
`<name>.key` is the record's key, as in the aggregate, `<name>._depth`
its depth and `<name>._type` its type; none is ever probed. An order probes its field before a row is read (024). A
projected field is named, not computed over, so it is not probed. A
field its reader may not know of is refused as one the type does not
have. A masked one is left out of the row, as an absent value is: a row
holds values, and a masked field has none to give. Rows are sorted
before `fields` is applied, so an order may read a path the projection
leaves out.

An `order_by` or a projection's `fields` that read the text before its
last dot as a type would sort by nothing, or keep nothing, when no type
of that name exists, or the pattern does not bind it, or the path runs
past a field into members. In both cases the answer would look as if it
had read something. An order by `<type>.key` over a type with no field
`key` would be refused, though every row holds that path.

Two faults in one pattern are answered in one order on every lake: each
order's form and its `by` are judged, then each order's path is read,
then each order is probed, then each projected path is read, then each
projected field is judged. The first fault in that order is the answer.

### A whole-graph measure is a projection

Degree centrality, connected components, PageRank and communities are
projections of the one query, `degree`, `components`, `page_rank` and
`communities`, not doors of their own. The start picks the records
measured; each step names a relation and a direction. The answer is
rows with fixed columns (`type`, `key`, `degree`; `component`, `size`,
`type`, `key`; `type`, `key`, `page_rank`; or `community`, `size`,
`type`, `key`, numbered and ordered as components are), paged like any
rows, so a caller learns no new result shape. A measure refuses a step
`depth`, `where` or `type`, `to` and `order_by`, and `components` and
`communities` refuse a step that does not walk `both`: each would be
ignored otherwise, and an answer that ignored what it was asked would
look like more than it is. A measure reads every link of a relation,
whatever type it reaches. The refusal of a step's `type` is judged
before the step's type is looked up, so it says nothing of whether that
type exists. Components, PageRank and communities hold every record
they reach, so the lake bounds them by its `max_graph_nodes`; past it
they are refused `too_large`, naming the ceiling, for the reason a step
too deep is refused: a partition or a ranking of part of the graph
would look like the whole of it.

`page_rank` and `communities` are accepted (lakelands#1412); `page_rank`
is built for 0.20.0 and `communities` for 0.21.0 (lakelands#1514).

`page_rank` ranks the graph its starts reach over the steps' relations,
each walked in its own step's direction (`both` walks a link each way),
closed under those walks, so the far end of every link it counts is in
the graph. Its rows come by `page_rank`, highest first, then by type and
key. Damping, the number of rounds and the starting mass are the lake's
fixed rule (026), and no member of the query sets them: a rule a caller
tuned would make two reads of one lake rank alike only where they had
tuned alike. A start with no link is a record of the graph that sends
its mass to every record evenly, as every record with no link out does.

`communities` partitions the graph `components` walks into groups more
linked within than between, each connected (026). Its answer is a
function of the graph and of the records' keys, never of the engine, but
where two partitions are equally good the keys decide between them, so
renaming a key may move a record from one community to another.

`components`, `page_rank` and `communities` alone may leave their
start's `type` out.
A host that asks for the components of the graph a set of relations
spans, across every type that holds such a link, would otherwise ask
once per type and merge partitions it cannot number by the lake's rule
without computing them again; only the lake can number the merged
partition, and only the lake can rank the merged graph, since a
record's rank moves with every record linked to it. With no type, every
record that holds a link the caller may see over a step's relation is a
start, in every type it is shown that declares one; a link's owning end
is always in such a type, whatever carries it. A record with no such
link is no component and no record of the graph, and counts nothing
against `max_graph_nodes`, and neither is a record whose links reach
only a record the caller may not see, which answered alone would tell
of that link; a record linked to itself is a component of one. Any
other projection needs the start's type, and so do a `key`, a `where`,
a `search` and `params`, each of which picks records of one type: each
is refused `start_no_type`, naming what needs it. A step's relation is
one that a type the caller is shown declares, or that relation's
`inverse`; another is refused `no_relation_anywhere`, whether it is
absent or hidden.

### What each step reaches is a projection

`reach` answers, for a chain of steps, what each step reaches: step `i`
walks from every record step `i - 1` kept, the start for the first, and
a record no later step continues from stays in the answer. Rows are
matches, a record for every step; rows that held a prefix would make a
match mean two things and repeat a record once for every path to it, and
a graph neither pages by step nor says which step reached a record. So
`reach` is a projection of its own, as a measure is: rows with fixed
columns `step` (from 1), `depth`, `type` and `key`, one per record, at
the first step that reaches it with that step's fewest hops from any
record it walked from. The start is listed only where a step reaches it:
at depth 0 where that step's range begins at 0, and at the hop a walk
comes back to it, as a chain reports a cycle back to its start; a step
walked `both` comes back over its own link at 2. Each step honours its
`depth`, `type` and `where` as a chain's does. Its order is `(step,
depth, type, key)`, so it refuses `order_by`, and `as`, which names no
column of it, and `to`, as every projection but a graph and `cheapest`
does, and a reach of no steps, which could never answer a row, as a
measure of none is. It holds every record it reached, so a page whose
walk would pass the lake's `max_graph_nodes` is refused `too_large`; a
page that stops earlier is answered, because a reach row is exact
whatever follows it.

### A view's records are computed when they are read

A view type's records are computed when they are read: the source's
records at the read's position, filtered by the view's `where`, grouped by
its `group_by` fields — whose values, in declared order, form the view
record's key by the same rule as any record's key (`docs/log-format.md`
§2.3), so a `group_by` field is of a kind a key field may be — and
measured, each measure landing in the kind the view declares for it. A
group with no member has no record. A view without `group_by` is the
exception: it is a total, and a total over nothing is zero rather than
absent, so it always holds exactly one record, the measures over every
source record its `where` keeps, even when it keeps none (a `count` and
a `sum` of none are 0, a `min`, `max` or `avg` null, as for an
aggregate). The key rule over no fields would make the empty string,
which is no key, so that record's key is the reserved `all`, the same on
every engine and at every position; `get` reads it by that key, and a
host that must tell "nothing matched" from "summed to zero" declares a
`count()`.

Nothing is written, so the log format does not change. Keeping a view's
records is the fold's, where a setting asks for it (049): it may make a
read faster and may never change its answer, and the conformance checks
on views bind it.

Every read of a view reads those computed records, the pattern query
included: a pattern that starts on a view, or a walk that reaches one
over a derived relation, finds the records `find` would, and never asks
an engine for records it does not keep.

### A view declares the fields it computes and no other, and its `key` names its `group_by`

A view declares the fields it computes, one for each `group_by` field
and each measure, and no other: a read answers declared fields only, so
a computed value with no field would be dropped without a word, and a
field nothing computes would always be empty; `declare` refuses both,
naming the field.

A view's `key` says what its records are keyed by or is left out: it
names its `group_by`, field for field in order, and an ungrouped view
names none. A `key` that named other fields would key nothing: the
records would carry their groups all the same, and a rule that reads a
type's key would judge a field no record carries as one. `declare`
refuses it at the type, and the offline bundle check says the same. The
form keeps `key`, since `describe` names a view's key and what it writes
declares again.

### A view is not searched, and a declaration that could never act on a view is refused

A view is not searched: the indexes a search reads hold stored records
only, so a search of computed records could only answer nothing. A view
field that is searchable, carries an embedding or is a vector is
refused when the view is declared, and a search on a view is refused,
naming it; neither is answered empty. For the same reason a declaration
that could never act on a view is refused when declared, naming both.
An object set holds stored records, or the groups of a view the lake
keeps, decided at every write that moves one, and a trigger on a view
the lake keeps fires at every write that puts a group's row
(`docs/decisions/066-a-set-holds-a-kept-view.md`); of a view the lake
does not keep no write would decide the one or fire the other. A
relation on a view is refused unless it is derived from matching values. A link names a live,
written record as its `from` (035), so a relation carried by link
records could never hold an edge. A view has no stored field for `via`
to read. The refusal names the view and the relation.

### A view is refused alike at every door that writes or holds a record

A view's records are computed, never written, so every door that writes
or holds a record refuses a view in the same words, as `invalid` naming
it: `upsert`, `retire`, `link` and `unlink` from a view, `ingest`, a
proposal of an upsert or a retirement, and the host's `restrict` and
`unrestrict`. Each door refuses it where it first knows the type: after
the type is looked up, before its grants or any record.

A verb on a record of a view is refused when it is declared, naming the
verb and the view, because it could never act: a write effect has
nothing to write, and an external effect finds no stored record to hand
its executor. Handing it a computed record would be a new capability,
not a fix. A verb on the view's type that reaches outside the lake can
act: it holds no record and writes nothing to the view. It is taken. The
declaration's refusal (`verb_on_view`) is the one place that holds the
rule: no door can declare such a verb, so a call, a proposed call and
its accept do not look for a view. Only a lake declared before 0.8.0
could hold one, and none is kept.

### A view may take parameters, and the read binds them

A view may declare parameters, each a name and a scalar kind (text,
number, decimal, boolean, date_time or date), and read them in its
`where` and its measures as `param('name')`. The read binds them: `find`
and `get` on the view, and a pattern that starts on it, carry `params`,
and every declared parameter must be bound. One view then answers the
window around any date its reader names, which is the reader's question
and not the view's: a thirteen-week cash flow anchored on the date a
person asks about is one view, not one per date.

There is no default. A parameter a read forgets is refused, naming it,
because a default would answer a window around a date nobody asked for,
and that answer would look complete. A reader who means today passes
today. A date given without a time is midnight UTC, so
`date <= param('anchor')` leaves out the rest of that day; a reader who
means its end passes the next day with `<`, or the instant itself.

`param()` is read only in a view's `where` and measures, where the view
declares what it reads; anywhere else it is refused. A parameter is a
value, never the clock: `today()` and `now()` stay the read's time, and
`as_of` and `valid_at` combine with the parameters as they combine with
each other. A view's parameters are part of its shape, so changing them
is a change of shape like any other. A view that declares none keeps the
schema hash it had. Views are computed on read, so nothing is kept per
binding.

### A pattern binds only the view it starts on

A pattern binds only the view it starts on. Suppose one of its steps, in
rows or in a measure, arrives at a view with parameters other than its
start. That view has nothing to read it with, so the pattern is refused
before any record is read, naming the view and the relation. Refusing
only once the walk had reached a record would make the answer depend on
the data, an empty answer with no records and a refusal with some
(029). A derived relation to or from such a view stays declarable: a
pattern that starts on the view binds it, and may step out and back.

### A view may merge sources on its key

A view may read more than one type: beside its `source` it may `merge`
further sources, each with its own `where`, its own aggregate measures,
and a `group_by` of its own fields that lines up with the view's by
position. An energy balance reads meals and workouts, which know nothing
of each other, and nets them by day. The groups of the sources meet where
their keys are identical, by the identity of the values, as a derived
relation's match does (044), not by 017's `=`: the text `010` and the
text `10` are two keys, and a date written as a day and as midnight of
that day is one.

Every key any source holds is a record of the view. A source that lacks
a key measures its empty group by the aggregates' own rules, so a sum
there is 0 and a minimum is none; a balance has a day with meals and no
workout without a declared default. The view's own measures come after
the merged ones and read them by name. The view's parameters bind every
source, and each source is read as a read of that source, so a field a
principal may not read refuses the view (024). That refusal is not
granted and names neither the field nor its source to a reader who is
not shown the view's definition (024, extended). A source may
not be a view with parameters, since only a read that names a view binds
them, and a view may not read itself through any of its sources.

### A cursor continues only the read it was handed out for

A cursor carries the parameters its first page was read with, and
continuing it with others is refused, since the pages would belong to
two answers. For the same reason a cursor carries the door and the type
it was handed out for: a page of `find` continues only that `find` on
that type, a page of `query` that query, a page of `search` that search,
and any other use is refused as invalid, naming where the cursor
belongs. A cursor of `find` holds one sort value per key of the order
its first page was read in; handed to a `find` with more or fewer keys,
it is refused as invalid before anything is compared. It also carries a
digest of the read it continues: for `find` its `where`, `order_by` and
`valid_at`; for `search` its mode with every member, its `where` and
`valid_at`; for `query` the whole pattern but its cursor, its `limit`
and `as_of`. A read with another digest refuses the cursor as invalid,
so a page taken by another filter, order, search or pattern is never
answered as if it continued the first. The page size, `with_meta` and
`total` may change from page to page; `as_of` is the cursor's own. The
cursor's bytes are opaque and change freely; no log format changes.

## Why

A graph read that returns nothing because of a misspelt relation looks
exactly like a graph read that found nothing. Refusing undeclared steps
turns a silent wrong answer into a clear one.

Keeping one query with named special cases, rather than one door per
graph operation, keeps the tool list short (014) and gives every graph
read the same time axes, paging and rights. Translating any later
language into it, rather than planning that language separately, means
masking, rights and time behave identically whichever syntax a caller
used.

## What it is not

- It is not a general graph database. Depth is bounded by the lake's
  limits, and a step walks only declared relations.
- Aggregation across pattern steps is not in `0.1.0`; aggregation of the
  matches is.
- `search` as a start does not make the pattern query a search door;
  `search` stays its own ranked door (014).

## How it came to be

- The first version: walking relations is one query with its named
  constructions, a planner that knows the ontology, and no second
  planner.
- Too deep is refused, not shortened, and a `subgraph` pattern's steps
  are one walk, not a chain.
- A view's records are computed when they are read.
- `degree` and `components` joined the projections (lakelands#149).
- `components` may leave the start's `type` out and grow from every
  linked record (lakelands#992).
- `reach` joined the projections (lakelands#991): a host walking a path
  of relations asked one rows query per prefix and compared them.
- A pattern that starts on a view, or a walk that reaches one, reads its
  computed records (lakelands#281); a view is not searched
  (lakelands#290); a view declares exactly the fields it computes
  (lakelands#310).
- A view may take parameters bound at the read (lakelands#148) and may
  merge sources on its key (lakelands#278).
- A view without `group_by` holds one record, keyed `all`
  (lakelands#346); a trigger, an object set and a relation that is not
  derived are refused on a view (a set and a trigger on a kept view are
  taken since lakelands#1185); a step that arrives at a view with
  parameters is refused before any record is read.
- Every door that writes or holds a record refuses a view alike, and a
  verb on a record of a view is refused when declared. Until 0.8.0
  `retire` answered `not_granted` to the owner and `restrict` answered
  `not_found` for a record `get` returns, each telling the caller
  something untrue (lakelands#370).
- A cursor carries the door and the type it was handed out for
  (lakelands#367), one sort value per key of its order (lakelands#436)
  and a digest of the read it continues (lakelands#419).
- A view's refusal over a field its reader may not read names neither
  the field nor its source to a reader who is not shown the view's
  definition.
- A graph answers what its steps say: a neighbourhood where every step
  starts at depth 0 and a chain otherwise, a graph cut by `limit` pages
  by its cursor, and a path walks its steps' relations. Before, a path
  with no steps walked every relation one hop (lakelands#556).
- A pattern's aggregate reads its rows by their paths. Until then a
  measure's field named a path whole and a group key did not: a named
  key read `bill.amount` as the field `bill` and then its member, a
  field key by its path was refused by the rule for a named key's name,
  so a pattern could group only by a setting or a constant, and a name
  that named nothing answered quietly (lakelands#720).
- An order and a projected field read their paths by the aggregate's
  rule; until then each read the text before its last dot as a type. A
  step that reaches several types bound each of them, where a chain's
  step had bound only the first type that declares its relation
  (lakelands#752).
- Each record a row binds has a name of its own, where a row had bound
  each record under its type's name and a second record of one type
  overwrote the first; a pattern whose steps each reach one type, and
  none of them a type twice, answers as it did. A step whose records are
  bound reaches one type over its whole range or names its `type`, which
  replaced the binding of each type such a step reaches; a chain's graph
  step that another step follows does too, where the next step had been
  read from the first of the types. A measure's step `type`, until then
  checked against one type the step reaches at most and never applied,
  is refused. A `type` a step does not reach is refused, where it had
  answered as if the step had reached nothing (lakelands#680).
- A call, a proposed call and its accept stopped looking for a verb on a
  record of a view: the declaration's refusal alone holds the rule
  (lakelands#823).
- A view's `key` names its `group_by` or is left out; until then a
  `key` that named other fields went in and keyed nothing
  (lakelands#788).
- The sentence that named the log format a merging view is written in
  left: a lake is written in one format a release (lakelands#801).
- A pattern is refused for what it says: four things it could say were
  passed over, each answered as if it had not been said or only where
  the lake happened to hold a record. A host that sent an `order_by`
  with a graph was answered as if it had sent none. A step whose range
  is empty or goes past the lake's `max_depth` was refused by the walk,
  once for each start, so rows and a path from a start no record has
  answered nothing. A neighbourhood over a relation no type its reader
  is shown declares answered the start alone. What a step with a `type`
  reads and a step by an inverse walks is pointed to, in 024 and 044
  (lakelands#829).
- Corrected at the rewrite (lakelands#817): keeping a view's records
  was "an engine's optimisation, never a rule"; the fold keeps them
  where a setting asks, which 049 added.
- Corrected at the rewrite (lakelands#817): a view's parameter may be of
  kind `date` too, which the list of kinds left out.
- A chain's last step that reaches several types probes the valid time
  of each at a `valid_at`; until then "nothing is read from them" stood
  for the valid time as well, and the walk filtered by fields nobody had
  probed (lakelands#831).
- An order asked for and not given: a pattern's rows ordered by
  `seen_at` or `known_at` came in the order of the value, and an
  `order_by` with an aggregate was passed over. Both are refused, and
  an order of rows is judged before the walk and before its path is
  read: a well-formed `latest` answers `latest_no_change` and a key
  without a field `order_names_no_field` where both answered
  `pattern_no_path`, and `latest` with a field `latest_takes_no_field`
  where it answered `latest_no_change` (lakelands#901).
- A row says the hops each step took, `<name>._depth`, and a graph each
  node's depth (lakelands#989): the walk computed the depth and ordered
  by it, and the answer dropped it, so a caller asked one query a depth
  to learn it. A new field may not begin with `_`, the lake's names
  under a bound name, and a field a type already holds keeps its name.
- A last step over several types is bound where nothing reads its fields
  (lakelands#990): under its relation's name, with each record's key,
  `_type` and `_depth`, counted over every type and grouped by `_type`,
  where every such step was refused and a caller asked one typed query a
  type. `step_several_types` names the types. 018 named every `_` name
  under a bound name the lake's column, where only `_depth` and `_type`
  are; a field an older lake holds under another `_` name is bound as a
  field.
- An object set may hold the groups of a view the lake keeps, decided
  at every write that moves one; before, every set of a view was refused
  (lakelands#1185).
- Patterns that branch and join were accepted as
  `docs/decisions/080-a-pattern-may-branch-and-join.md`
  (lakelands#1404): a tree of steps, a join that closes a cycle, an
  optional and an absent step, for rows and an aggregate.
- A query language that writes only through the typed write doors was
  accepted as
  `docs/decisions/082-a-query-language-writes-only-through-the-typed-write-doors.md`
  (lakelands#1404), for 0.21.0.
- PageRank and communities were accepted as measures and the cheapest
  walk as a path answered as rows (lakelands#1412); `page_rank` may
  leave its start's type out, as `components` may, and the rule that a
  pattern with `to` answers no rows gained `cheapest` as its exception.
- A cheapest walk judges `path_starts` after its steps and its weight
  fields, where `path` judges it, not before them: several starts are
  known only once a filter start's records are read (lakelands#1422).
- `communities` was built and, as `components` and `page_rank` may, may
  leave its start's type out; a step that does not walk `both` is
  refused `components_both_ways`, whose words now name both measures
  (lakelands#1514).
