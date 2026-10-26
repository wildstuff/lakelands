# 066 — A set holds a kept view

This record extends `docs/decisions/049-a-view-the-fold-keeps.md` and
`docs/decisions/055-a-path-steps-through-a-reference.md`.

## What is decided

### An object set may hold the records of a view the lake keeps

An object set's `of` may name a view the lake keeps (049): its members
are the view's groups, by their keys, and its `where` reads the view's
fields. A set of a view the lake does not keep is refused
`set_of_view`, naming the set and saying to set `view_kept.<view>`
first. Whether a view is kept is a setting's, so this is judged where a
set of declarations is decided, not by the declaration's own check: a
proposal of such a set is listed, and its accept is refused while the
view is not kept.

### A membership is decided at the write that moves the group, by the fold's own code

The fold never appends (049), and a membership lands in the append of
the write that moves it, caused by that write's event
(`docs/decisions/004-the-change-feed-is-the-log.md`). So the write path
computes the view's rows itself: at every write of a record a set's
view reads, through every door that writes one (`upsert`, a verb, an
accept, `ingest`, `retire` and revival, a purge, a source forgotten, an
aging move, a backed field's fill), it runs the fold's own hooks over a
transaction of its own and no projection, with the records, the
settings and the earlier writes of the append the fold will read. Each
group those hooks put or end at the write's event is decided for every
set of its view; a view of that view is told through the same hooks, so
a set over the outer view is decided at the same write. A group that
ends leaves every set.

- **Exact by construction.** One code computes a group's row at the
  write and in the fold, from the same head and the same earlier writes
  of the append. The fold is unchanged.
- **The whole face.** A set is decided for no reader (004): over the
  group's whole face, restricted members included.
- **A group the fold flags as not exact** (049: a measure that fails, a
  sum outside exact arithmetic, a member that does not fold), or one of
  a view whose inner view has such a group, is decided on what a read of
  the view computes for it, from its members as the append leaves them.
  A group whose measure fails, which a read of the view refuses, is a
  member of no set; so is every group of a view over it while a group it
  reads there fails, since a read of the outer view refuses too. A write
  that touches a group flagged before it or after it decides every group
  of each set's view that reads it: a flag that stays does not say that
  the failure it stood for has cleared. The cost is a pass over those
  groups at each write to a flagged group, which is rare; a write to an
  exact group pays none of it.
- **A write refused** is undone in the append's run of the hooks: what
  its run put there is put back as it was, and the next write runs over
  the writes accepted before it, which are not run again.

A restriction moves no whole face, so a hold placed or lifted decides
nothing.

### Where the view is made again, every group is decided in that append

A set of declarations that declares the view or one of its sources
again, a set of a view declared or declared again with another
condition, and a write of a setting the view's expressions name make
the view again in the write path as the fold will make it, over the
head's records and under the declarations and settings the change
leaves, and decide every group of every set of it, caused by the
change's own event: the last declaration of the set, or the setting's
write. A set declared over a view decides its members from the view's
kept records at the head, as a set of a stored type reads its records.

A write of a setting a set's `where` names decides every group of that
set in its own append, under the value written, caused by the write's
event; so does a set of declarations that stamps such a setting again.
A membership answers what a read's `in_set` answers (below), which reads
the setting as it is.

### A set's `where` does not read the clock unless it is decided at the read

A set keeps the memberships it decided at the write, and nothing writes
again when a clock passes a boundary; a read's `in_set` evaluates `now()`
and `today()` at the read's clock (052). A `where` that read the clock
would let the two drift apart as time passes, so a kept set whose
`where` calls `now()` or `today()`, at any depth, is refused
`set_reads_clock`, naming the set and the call, whether it is a set of a
type or of a kept view. No ticker re-decides memberships as time
passes. A set whose membership moves with the clock is declared decided
at the read (below), which keeps no membership to drift from; a host
may also keep a time window in a view's `where`, which a read evaluates
at its own clock, or in a computed field. A trigger's `when` is not
touched: it is decided once, at the write that fires it.

The check is made where a declaration is judged. A log written before
holds such a set and opens and replays as it did, as a kept set; the
lake meets the refusal only when it declares that set again.

### A set may be decided at the read

A set declares when it is decided, `decided_at`: `write`, which it is
unless it says otherwise and which keeps its members as this record
says, or `read`. It is accepted for 0.20.0 (lakelands#1412). A host
asked which of the records it shows a condition reading `today()` rules
out, by a name a model can carry, and a kept set could not hold it.

- **It keeps no membership.** No write decides it, the fold holds no
  member of it, and no `object_set_changed` is appended for it, so
  nothing of it is in the feed, a subscription or the bound
  `max_set_changes` counts.
- **It is expanded at the read.** `in_set('<set>')` of it, in a read, is
  its `where` over the record read, evaluated at the read's clock (052):
  two reads at two clocks may answer differently, and each answers what
  a `find` with that `where` answers at its own. Its `where` may call
  `now()` and `today()` and may name a field computed at the read.
- **It steps through no reference.** A read evaluates no path through a
  reference (055), so a `where` that steps across one, or calls
  `exists`, is refused, naming the set and the path. A kept set it
  names by `in_set` answers by its kept membership, whatever that set's
  `where` reads; the set decided at the read steps through nothing by
  naming it.
- **Nothing decided at a write names it.** A kept set's `where`, every
  view's `where` and its merged sources' `where`s, kept or not, a
  trigger's `when` and a verb's `requires` that name it by `in_set` are
  refused, naming the set and what names it: what a
  write decides by it would drift from what a read of it answers, the
  reason a kept set does not read the clock.
- **Declared again, it changes how it is decided.** A kept set declared
  again as decided at the read ends its members in that append, as a
  withdrawal does; one decided at the read declared again as kept
  decides the records already there
  (`004 § A set's declaration decides the records already there`), and
  is refused if its `where` reads the clock.
- **Its rights are a named set's.** It is read by its filter's right
  (`006 § A named set is read by its filter's right`), and `describe`
  shows `decided_at`.

### What would leave a set of a view without its rows is refused

While a set holds a view, through any view it reads too:

- ending the view's keeping is refused `view_kept_for_set`, naming the
  set: its setting written `false` or retired, and a declaration under
  which the view cannot be kept (049); the to-do is to withdraw the set
  first (lakelands#1219);
- a change that moves a value the view reads where no write decides the
  set again is refused `view_source_stepped`, naming the set, the view
  and the source. The view reads the fields its sources' `where`,
  grouping and measures read. Such a change is a migration that takes,
  renames, declares otherwise or fills such a field, or any field of a
  source where one it reads is computed; one that re-keys a type a read
  reference or a read link's end names; one whose relation's change
  moves the records of a link type the view reads; a value type's rename
  under a read field; and a set of declarations that computes a read
  field again. The to-do is to withdraw the set, make the change, and
  declare the set again, whose declaration decides its members. A
  migration that moves no value the view reads lands: the fold makes the
  view again from the same values, and no membership moves.

A plan of such a change (062) refuses it alike, on the same path; a
proposal's plan tells it not, as it tells no set's decision, and its
accept is refused. To an
author not shown the set the refusal names no set: one that moves a
source is refused `no_longer_declarable`, naming the source the author
is changing, and any other `not_granted`.

### A trigger on a kept view fires where a group's row is put

A trigger's `on` may name a view the lake keeps (lakelands#1185), as a
set's `of` may; on a view the lake does not keep it is refused
`watches_view`, naming the trigger and saying to set `view_kept.<view>`
first, judged where a set of declarations is decided, as a set's is.

It fires from the run of the fold's hooks that decides the sets: at a
write, for each group the hooks put, where the group arrived or a field
of its row on the whole face moved, and its `when` holds over the row
after. `previous` and `changed` read the row before; `created()` holds,
and the firing says `created`, where the group had no row on the whole
face before, again after it ended. A group that ends fires nothing:
there is no row after. Where the fold had flagged the group as not
exact, or a view it reads through holds such a group, the row before
is none, since a read computes that face from members the write has
moved since: a trigger there fires as on a group that arrived, except
that the firing does not say `created`. The guard reads the settings as
they stand before the write, as at a record, while the row after is
computed under the settings the write leaves. The firing names the view
and the group's key, is caused by the write's event, lands after the
sets' memberships, as across an edge, and says the write's kind of
change; `on_changes`, `when_strict` and the depth bound (021) apply as
at a record. A write of a setting a view's
measure names puts every group's row again, and fires where a row
moved. A set of declarations that makes the view again fires nothing:
a trigger fires at a write, as at a record.

What would leave the view without its rows is refused while a trigger
watches it as while a set holds it, naming the trigger
(`view_kept_for_set`, `view_source_stepped`; their to-do says to
withdraw it first). Its firings count toward the bound, and the trigger
is a reader's only where nothing of a source is held from it, by the
same rule as a set: its `trigger_fired` is told to no other reader.

### The bound counts them

Every membership these decisions move, and every firing on a view,
counts toward the lake's `max_set_changes` (055), with what the same
append moves across an edge, and so does every group a remake moves.
Past it the write is refused `too_large`, naming the set or the trigger
to a caller shown it and nothing to another, and nothing is appended.

### A set of a view is a reader's only where nothing of a source is held from it

A membership over a group's whole face tells what every member adds,
restricted ones and another's among them. So, by 055's rule, a set of a
view is a reader's only where the reader holds `restricted`, holds no
`only` on a source of the view at any depth (merged sources and the
ends of a source that is a link type included), and reads every field a
source's `where`, grouping and measures read. To any other reader the
set is one that is not declared: `in_set` of it is `set_undeclared`,
`describe` names it not, and its events are not told to the reader's
subscription. A writer is told that such a set moved only where it is
answered the view's group. No membership is kept per face: a reader with
an `only` would need one for each filter.

### A read's `in_set` reads the set's condition over the view

In a read of the view, `in_set('<set>')` of such a set is its condition
over the records the read answers. The set is shown only to a reader
who holds `restricted` and is held nothing of a source, whose read
answers each group's whole face: the groups its memberships hold, at
the read's position and as of an earlier one. So it needs no read of
the memberships, as a set across an edge does (055).

## Why

A host reacts when a computed row starts or stops meeting a condition:
a customer's balance passes a limit, a stock's total runs low. Without a
set over the view, the host reads the view after every write and keeps
the last answer itself, a second source of truth for what the lake
already computes.

The membership cannot come from the fold: the fold never appends, and a
membership is an event of the write's own append. Computing the rows in
the write path by any other code would be a second definition of the
view that drifts from the first; running the fold's hooks over the
write's own records is the definition itself.

The refusals take the cases where the write path does not hold the
records the fold will read: a migration's step and a computation of a
field again move records the write path never sees as writes. Refusing
them while a set holds the view keeps every membership exact; deciding
them would mean running a migration twice.

## What it is not

- **Not free.** A write of a record a set's view reads pays the view's
  kept share in the write path too: about what the fold pays for the
  same write, since the hooks read the group's kept row and an
  extreme's order entries. At an `ingest` its reads of kept rows and of
  the views' states are handed to the fold, which does not read them
  again. The hooks run the views a set holds and those they read
  through, and no other kept view. A write refused costs the writes
  before it nothing. A write of any other type pays nothing more. A
  remake reads every record of the view's sources, as the fold's make
  does, in the change's own append. A group the fold flags as not exact
  is the exception: deciding it reads every record of the view's
  sources, at each write that moves it, as a read of the view computes
  it while the view holds such a group. A write that decides every group
  of a view (a setting a set's `where` names, or a write that turns a
  group the view reads flagged before or after it) pays that for each
  group while the view reads such a group. A trigger on a view makes
  the writes of its sources pay the same as a set of it would.
- **Not per face.** A reader who is not shown the set reads the view,
  and filters it by its own `where`, as before.

## How it came to be

- A set may be decided at the read, keeping no membership, and the
  refusal of a `where` that reads the clock became a refusal of a kept
  set's (lakelands#1412): a host asked which of the records it shows a
  condition reading `today()` rules out, by a name. The operator
  accepted it on 2026-10-25.
- What names a set decided at the read is refused for every view's
  `where` and its merged sources', kept or not (lakelands#1441), and a
  kept set a read names by `in_set` answers by its kept membership,
  whatever its `where` reads (lakelands#1437).
