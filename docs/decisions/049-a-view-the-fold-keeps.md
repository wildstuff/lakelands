# 049 — A view the fold keeps

## What is decided

A view asked for by the setting `view_kept.<type>` has its records kept
in the projection by the fold, one for each group, and a read of the
view answers them. **A kept record is exactly what the read would
compute, or the view says at that position that it is not, and the read
computes.** Nothing kept imitates an order the computation takes; the
computation at the read stays whole and answers wherever the kept form
would have to reproduce one.

- **Asked for by a setting.** The `sys.setting` record
  `view_kept.<type>` with the value `true` asks for the view to be kept.
  A write of it for a view that cannot be kept is refused, naming the
  measure and why. Retiring it, or any other value, asks for nothing.
- **What can be kept is read off the declaration.** A view with no
  parameter and no clock, each of whose aggregate calls is a function of
  its values as a multiset: any `count`; a `sum` or an `avg` of what is
  certainly a number; a `min` or a `max` of what is certainly a number
  or an instant, and that only under group fields that are all text or
  references, one at least, so that a group whose extreme left is found
  again by its fields' values. A view over a view is kept while that
  view is.
- **Under a name no type can bear.** A view's kept records are put under
  `sys:kept.<view>`, which holds a character no type name may. No write,
  link, walk, preflight or fold reaches them by a type's name: a
  reference to a view is refused kept or not, as before. Only the view's
  own read asks for that name. Its state is recorded under the seam
  `sys:kept`, and every seam beginning `sys:` is the store's own. A log
  that names either, or writes a record of a view, comes from no door
  and is not folded past that event (lakelands#621).
- **One record a group, with its folds.** A kept record holds the
  group's fields as the view's record holds them, and beside them, in a
  field no read answers, what the group keeps in place of its members
  for each source of the view: counts, sums by scale as integers, an
  extreme with its holder's key. The fold changes a group by the member
  that arrives, leaves or changes, and never reads the group's other
  members, with one exception that is bounded (below).
- **Two faces.** A restricted record is a member for a reader with the
  `restricted` grant and for no other. A group keeps its folds for both:
  the record's own fields are the open face's; where the whole face
  reads otherwise, the record holds its fields too, and the reader with
  the grant takes them. A group whose members are all restricted has a
  restricted record, which the engine leaves out for every other reader.
  A group can be on the open face alone as well: a view over a kept view
  takes each face of an inner group as that face's member, and its
  `where` can hold over the open one and fail over the whole one. Its
  record says so, the reader with the grant is answered none of it, and
  it counts among the groups that read otherwise on the whole face.
- **A state of counters.** For each kept view the fold records, at the
  position of the event that changes them: whether its records are
  whole, how many groups are not exact on each face, how many read
  otherwise on the whole face. A read
  answers the kept records where the state at its position says whole
  and counts no group that is not exact for the reader's face, for the
  view and for every view it reads through, and where the read names no
  valid time; it computes otherwise. So a refusal, its sentence and its
  order are always the computation's own.
- **What is not exact is counted, never returned.** A measure that
  fails over a group (a division by zero), a member whose argument does
  not fold, a sum outside what exact arithmetic holds, an extreme given
  up (below): the group is flagged and counted, its other folds go on,
  and it is exact again when the cause leaves; a group given up, when
  the view is made or the group ends. Noting a member, putting a group
  and making a view return no error but the engine's, so no view can
  stop the lake.
- **Made by noting from nothing.** A view becomes kept in the fold of
  the event that makes it so: its setting, or a declaration under which
  it can be kept. Every record of its sources is noted as arriving, a
  page at a time, and every group is put once. The folds of every group
  of the view are held until the transaction that folds that event
  ends. A view stops being kept in the
  fold of the event that makes it so: its records end there, and those
  of every view that reads it.
- **An event that changes what a view reads under its records makes it
  again.** The view or a source declared again; a migration whose step
  reaches a source's records (the migrated type, the types whose keys or
  references move, the link types records move between); a write of a
  setting that an expression of the view names. The view is ended and
  made in that event's fold. Nothing is noted from inside a migration's
  step.
- **An extreme whose holder left is read again in the fold of that
  event, within a bound.** Before the event's groups are put, each group
  an extreme of which lost its holder is read again by the engine's
  lookup on its group's values, over 8 192 live records at most, and is
  exact before the next event is folded. A member that is a view's
  record is found by its own fields, and for the whole face by a mark
  where that face reads otherwise. The bound is one for the group over
  all the view's sources: a record the view's `where` leaves out counts
  in it, and so does a marked record of another group. After a
  migration's step in the same transaction the lookup is a walk of the
  type as the step leaves it, to its end.
- **Past the bound a group is given up.** Its extreme is not the
  computation's own from then on: the group is counted as not exact on
  the faces it concerns, the view computes for their readers, and
  nothing reads the group again, not even a later holder's leaving.
  What mends it is what makes the view, or the group's own end: a group
  whose last member leaves starts from nothing when one returns, in the
  same transaction as in a later one.
- **Every decision is one event's, but one.** What the fold does to a
  kept view it does in the fold of one event, on what the lake holds at
  that event: nothing waits past its event, and nothing depends on where
  a transaction ends. So a rebuild, a catch-up and an import leave the
  records and the state the live appends left. The one exception is the
  make after a declaration that computes a source's records again: it
  stands where those records are put, before the first later event
  that reads a record or at the transaction's end, so a rebuild whose
  page holds the declaration and later events makes the view at a later
  position than the live appends did, with the same records. As of a
  position between, `describe_type` may then say `not_kept_here` where
  the live lake said `kept`; every read of the view's records answers
  alike (lakelands#631).
- **An append pays for the views that read what it wrote.** The state
  of a kept view is read in when a record it reads is written, and all
  of them at a setting's write or a declaration; a lake with no
  `view_kept.` setting reads none, in an append or in a read.

## Why

A view's read walks its source: at a hundred thousand records a view
answers in seconds where a stored type answers in milliseconds, and a
host that shows a report cannot wait for it. The alternatives were a
table the host maintains (two sources of truth for one number) and a
cache of computed answers (stale by construction, or invalidated by
every write). Keeping the records in the fold makes the view's answer
part of the projection, at every position, with the log still the one
source.

The principle is what three rounds of review left standing. Each
attempt to make the kept fold reproduce the computation's order (of
keys, of sources, of probes, of scales) had a state it got wrong. So
the kept form answers only what is a function of the group's values,
decided by the declaration, and counts everything else; where the count
is above zero the computation answers, with its own refusal.

A setting and not a declaration, because whether a view is kept changes
no answer: it is the lake's configuration, as an order index is (041).

A group is read again in its event's own fold, and not at the end of
the transaction, because a transaction is what the fold commits: an
append when live, a page of events in a rebuild. A group given up at a
transaction's end would be given up by what the group held there, which
a lake and its rebuild do not share, and given up is for good. And it
is given up past a bound, not walked on over the appends that follow,
because a walk that goes on is a second state beside the folds, and
keeps the writer reading a group for as long as its holders leave. What
makes a large group's extreme cheap is the engine's own order for the
new holder, which is lakelands#629.

A migration makes the view again because a step moves keys, types and
values under every group at once; noting it record by record from
inside the step would be a second walk with the transaction's own
writes laid over, and the make is that walk, exact, in a transaction
that already walks the type.

## What it is not

- **Not the engine's page alone for every reader.** For a reader
  without the `restricted` grant, and for one with it while no group
  reads otherwise on the whole face, the page, the count and the single
  record of a kept view are the engine's, as a stored type's are; the
  filter, the order and the masking are the read's own. Once groups'
  faces differ, the reader with the grant is answered the engine's page
  of the others merged with those groups (below).
- **Not sub-linear in every read.** A page in another order than the
  key's, an aggregate and a pattern's start read the whole view for
  every reader (lakelands#668). A later page of a cursor walk, read as
  of its first page's position, finds the differing groups by scanning
  the kept type in key order: its memory is bounded by them, its time
  goes with the view; the text index that finds them serves the head.
- **Not kept at any size.** A view with an extreme over groups larger
  than the bound, or whose holders leave in bulk, is not worth keeping:
  each event that takes a holder reads its group under the writer.
- **Not for every view.** An extreme over a text, a sum over what may
  be a text, a view with a parameter or the clock compute, as before.

## Extended: the whole face is paged by the engine too

A kept record's own fields are its open face. Where the whole face
reads otherwise, the record holds that face inside its folds and bears
the mark `sys:differs`; the state counts those groups. So the reader with
the `restricted` grant reads the whole face as two streams. The engine
pages the kept records without the mark, restricted ones included,
under the reader's filter and order: their whole face is their own
fields. The few with the mark are found by it, each taken whole (a group
on the open face alone is dropped), filtered and placed by the store,
and merged into the page by the engines' one ordering. A cursor and an
`offset` walk the merge. A count is the engine's count of the kept
records less those with the mark whose own fields pass the filter, plus
their whole faces that pass. A single record takes its face from its
own folds.

A page in key order holds its rows and the differing groups; a count is
the engine's and those groups. The groups that differ are those with a
restricted member whose faces then read otherwise, which is the rare
case the counter is kept for; where most groups differ, the merge holds
most of the view, which is no more than the read held before.

**Why.** Before, once one group's faces differed, the reader with the
grant was answered from every kept record at each call: at 100 000
groups a page of 10 took 408 ms and 321 MB on SQLite, and `get` of one
group 324 ms and 321 MB, against under a millisecond for every other
reader. That reader is most often the lake's owner. A second stored type
holding the differing groups' whole faces, paged by the engine as well,
was weighed and left out: it adds a kept name whose records the fold,
a rebuild, an import and a purge must put and remove whenever a group
starts or stops differing, for a set the counter says is small.
