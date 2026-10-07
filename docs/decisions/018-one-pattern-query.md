# 018 — One pattern query, and never a second planner

## What is decided

Walking relations is one query: a start (a type with a key, a filter or
a search), a list of steps (relation, direction, target type, filter,
depth), and a projection (rows for agents, a graph for viewers, or
aggregates). `neighbors`, `traverse`, `path` and `subgraph` are named
constructions of it, not doors of their own. Every answer carries its
position and can be pinned with `as_of` and `valid_at`.

The planner knows the ontology. A step over a relation its type does not
declare is refused, not answered empty, and grants apply per step,
because every step has a type.

Any later query language a caller writes in (a read-only openCypher or
GQL subset, for example) is translated into this query. There is never a
second planner and never a write path through a query language.

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

## Extended: too deep is refused, not shortened

A step that asks for more hops than the lake's `max_depth`, or whose
`min` is above its `max`, is refused as invalid. It is never quietly
shortened to the limit: an answer that walked less than it was asked to
would look complete when it is not, and the caller could not tell. A
`subgraph` pattern's steps are one walk over any of their relations,
not a chain of steps.

## Extended: a graph answers what its steps say (lakelands#556)

A graph pattern whose steps all start at depth 0 is a neighbourhood:
one walk from its start over any of its steps' relations, each walked
in its own step's direction to its own step's `max`. A record reached
over a step's relation is kept, and walked on from, only where that
step's `type` and `where` hold of it. Since a neighbourhood walks into
types its start does not declare, a step may name a relation any type
declares. Its `where` is read against the type that relation reaches,
wherever it is declared: its target walked out, its owner walked in. A
step that may reach several types (walked both ways, or a relation
several types declare) and reads a `where` is refused as invalid unless
it names its `type`. Any other graph pattern is a chain, walked as its rows are:
each step from the records the step before it kept, with its own
relation, direction, depth range, `type` and `where`, and a step over a
relation its type does not declare is refused. A chain's graph is the
start, every record a match binds, the records each was walked through
from the record before it, and the links walked, so no link has an end
outside the graph. Several starts are one walk: each record is at the
fewest hops any start reaches it in. A step that names no depth is
`1..1`, so a graph of several such steps is a chain; a neighbourhood
names `0..` on each step.

A graph cut by `limit` says `truncated` and carries a `next_cursor`,
exactly as a page of rows does. The next page is the next `limit`
records of the one order (depth, type, key), with every link between a
record of that page and one of this or an earlier page, so the pages
together are the graph, each record and link once. Each page walks
again, at the cursor's position, as deep as its last record.

A pattern with `to` is a path: the shortest walk to that record, over
its steps' relations as a neighbourhood walks them, each in its step's
direction and only through records its step's `type` and `where` keep.
A path has one start, so a pattern whose start names several records
is refused as invalid, as is one with a step that does not start at
depth 0. A path is a graph, and a pattern with `to` that asks for rows
or an aggregate is refused as invalid: an answer that ignored what it
was asked would look like more than it is. Where no path is, the answer
is an empty graph. A path and an aggregate answer in one page and hand
out no cursor, so a cursor handed to either is another read's, and is
refused as such. A path or a neighbourhood with no steps walks no
relation: the neighbourhood is its start alone, and a path reaches no
other record. (Before lakelands#556 a path with no steps walked every
relation one hop.)

## Extended: views are computed on read

A view type's records are computed when they are read: the source's
records at the read's position, filtered by the view's `where`, grouped by
its `group_by` fields — whose values, in declared order, form the view
record's key by the same rule as any record's key (`docs/log-format.md`
§2.3), so a `group_by` field is of a kind a key field may be — and
measured, each measure landing in the kind the view
declares for it. A group with no member has no record. A view without
`group_by` is the exception: it is a total, and a total over nothing is
zero rather than absent, so it always holds exactly one record, the
measures over every source record its `where` keeps, even when it keeps
none (a `count` and a `sum` of none are 0, a `min`, `max` or `avg`
null, as for an aggregate). The key rule over no fields would make the
empty string, which is no key, so that record's key is the reserved
`all`, the same on every engine and at every position; `get` reads it
by that key, and a host that must tell "nothing matched" from "summed to
zero" declares a `count()`. Nothing is written, so the log format does
not change. Keeping a view's
records materialised is an engine's optimisation, never a rule: it may
make a read faster and may never change its answer, and the conformance
checks on views bind it. A view declares the fields it computes, one for each
`group_by` field and each measure, and no other: a read answers declared
fields only, so a computed value with no field would be dropped without
a word, and a field nothing computes would always be empty; `declare`
refuses both, naming the field. Every read of a view reads those computed
records, the pattern query included: a pattern that starts on a view,
or a walk that reaches one over a derived relation, finds the records
`find` would, and never asks an engine for records it does not keep. A
view is not searched: the indexes a search reads hold stored records
only, so a search of computed records could only answer nothing. A view
field that is searchable, carries an embedding or is a vector is
refused when the view is declared, and a search on a view is refused,
naming it; neither is answered empty. For the same reason a declaration
that could never act on a view is refused when declared, naming both.
A trigger on a view would never fire, since no write has the view's
type. An object set holds stored records, and a set evaluated over
computed records would be a feature of its own. A relation on a view is
refused unless it is derived from matching values. A link names a live,
written record as its `from` (035), so a relation carried by link
records could never hold an edge. A view has no stored field for `via`
to read. The refusal names the view and the relation.

## Extended: a view may take parameters bound at read

A view may declare parameters, each a name and a scalar kind (text,
number, decimal, boolean or date_time), and read them in its `where`
and its measures as `param('name')`. The read binds them: `find` and
`get` on the view, and a pattern that starts on it, carry `params`, and
every declared parameter must be bound. One view then answers the window
around any date its reader names, which is the reader's question and not
the view's: a thirteen-week cash flow anchored on the date a person asks
about is one view, not one per date.

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
schema hash it had. A cursor carries the parameters its first page was
read with, and continuing it with others is refused, since the pages
would belong to two answers. For the same reason a cursor carries the
door and the type it was handed out for: a page of `find` continues
only that `find` on that type, a page of `query` that query, a page of
`search` that search, and any other use is refused as invalid, naming
where the cursor belongs (lakelands#367). A cursor of `find` holds one
sort value per key of the order its first page was read in; handed to a
`find` with more or fewer keys, it is refused as invalid before anything
is compared (lakelands#436). It also carries a digest of the read it
continues: for `find` its `where`, `order_by` and `valid_at`; for
`search` its mode with every member, its `where` and `valid_at`; for
`query` the whole pattern but its cursor, its `limit` and `as_of`. A read
with another digest refuses the cursor as invalid, so a page taken by
another filter, order, search or pattern is never answered as if it
continued the first (lakelands#419). The page size, `with_meta` and
`total` may change from page to page; `as_of` is the cursor's own. The
cursor's bytes are opaque
and change freely; no log format changes. Views are computed on read,
so nothing is kept per binding.

A pattern binds only the view it starts on. Suppose one of its steps, in
rows or in a measure, arrives at a view with parameters other than its
start. That view has nothing to read it with, so the pattern is refused
before any record is read, naming the view and the relation. Refusing
only once the walk had reached a record would make the answer depend on
the data, an empty answer with no records and a refusal with some
(029). A derived relation to or from such a view stays declarable: a
pattern that starts on the view binds it, and may step out and back.

## Extended: a view may merge sources on its key

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
them, and a view may not read itself through any of its sources. A view
that merges is written in log format 9.

## Extended: a whole-graph measure is a projection

Degree centrality and connected components are two more projections of
the one query, `degree` and `components`, not doors of their own. The
start picks the records measured; each step names a relation and a
direction. The answer is rows with fixed columns (`type`, `key`,
`degree`; or `component`, `size`, `type`, `key`), paged like any rows,
so a caller learns no new result shape. A measure refuses a step
`depth` or `where`, `to` and `order_by`, and `components` refuses a
step that does not walk `both`: each would be ignored otherwise, and an
answer that ignored what it was asked would look like more than it is.
Components hold every record they reach, so the lake bounds them by its
`max_graph_nodes`; past it they are refused `too_large`, naming the
ceiling, for the reason a step too deep is refused: a partition of part
of the graph would look like the whole of it.

## Extended: a view is refused alike at every door (lakelands#370)

A view's records are computed, never written, so every door that writes
or holds a record refuses a view in the same words, as `invalid` naming
it: `upsert`, `retire`, `link` and `unlink` from a view, `ingest`, a
proposal of an upsert or a retirement, the host's `restrict` and
`unrestrict`, and a call of a verb on a record of a view. Each door
refuses it where it first knows the type: after the type is looked up,
before its grants or any record. Until 0.8.0 `retire` answered
`not_granted` to the owner and `restrict` answered `not_found` for a
record `get` returns, each telling the caller something untrue.

A verb on a record of a view is refused when it is declared, naming the
verb and the view, because it could never act: a write effect has
nothing to write, and an external effect finds no stored record to hand
its executor. Handing it a computed record would be a new capability,
not a fix. A verb on the view's type that reaches outside the lake can
act: it holds no record and writes nothing to the view. It is taken, as
it was before. A lake declared before 0.8.0 may still hold a verb on a
record of a view, since replay does not judge a declaration again; a
call of it is refused at the door as above.

## Extended: a pattern's aggregate reads its rows by their paths (lakelands#720)

A pattern's row holds each value under its path, `<type>.<field>` or
`<type>.key`, for every type the pattern binds: its start and the type
each step reaches. A measure's field already named a path whole. A
group key did not. A named key read `bill.amount` as the field `bill`
and then its member `amount`, found nothing, and grouped every row
under null. A field key by its path was refused by the rule for a named
key's name. So a pattern could group only by a setting or a constant,
and a name that named nothing answered quietly: a field key `zz`, a
sum of `zz` as 0.

Every name in a pattern's aggregate is now read against the paths the
rows hold. A field key and a measure's field name one whole. Each path
in a named key's expression is read as the longest type the pattern
binds, then one of its fields or `key`, then members by 028's rule for a
path. A field key answers under its path in the group's `by`. A name
that no bound type starts is refused, as a step over an undeclared
relation is and for the same reason: it reads nothing, and an answer
built on it looks like one that read something. The refusal is the same
for a type that does not exist and one its reader is not shown.

A named key is held to what `find` holds it to. Each field it reads is
probed before a value is read (024), so a field its reader may not know
of is answered as one the type does not have, as a field key on it is.
An operation it can never compute is refused (028), checked over the
row's shape: one field for each path, of its declared kind.
