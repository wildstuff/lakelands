# 048 — The fold keeps the event, not the records it changed

## What is decided

Two kinds of event change every record of a type: a `migrated`, and a
`declared` that computes derived fields again (a function declared again
with a change, a type declared again with another computed field, a
`recompute`). The fold of such an event walks the type a page of the
view at a time, puts each record it changes, and keeps none of them. It
keeps the event, as a **step**: what the event does to one record.

- **A later read goes through the steps.** An event later in the same
  transaction that reads a record the transaction did not write reads
  the view's row and takes it through every step kept so far, in the
  order of their events. A record the transaction wrote is read from
  what it wrote, and a step changes it where it is kept. A migration's
  step may move a record to another key, or a link to the link type of
  its relation's new name (035, extended): the view holds it where it
  was, and a recomputation names its types as they are after the steps
  kept before it.
- **A recomputation's step holds what its computation reads, as it stood
  at its event.** The event whole, since the fields it changes take its
  provenance member by member; the declarations of the types it computes
  and of the functions and value types their fields name, each shared
  with the ontology and not copied; the settings. A name the ontology
  did not hold then is one the step does not hold: such a field is not
  computed, by the walk or by a later read. A record of another type is
  as it was.
- **It reads the outputs recorded before its own event,** whenever it is
  asked. A transaction keeps every recording of a call with its
  position, so an output recorded after the event, even for the same
  call, is never read by it.
- **Its records are walked once for every declaration that is pending.**
  A `declared` that recomputes leaves its step pending. The pending
  steps are flushed before the fold of the next event that reads a
  record, and before the commit: each type is walked once, each record
  taken through the pending steps in order and put at every step that
  changes it, at that step's position. Which kinds of event read a
  record is said for each kind where the fold decides it; a new kind
  does not compile until it is said. A step that put no record is
  dropped.
- **A fold of the log ends its transaction after a flush that kept a
  step.** A rebuild, an open's catch-up and an import fold a page of
  1 024 events; such a page commits through the last event folded and
  begins again, so the events after it read the recomputed records in
  the projection. Every commit holds every event through its position,
  whole. An append's own fold is one transaction, always; an append
  whose fold failed is folded again as a fold of the log, which may
  commit inside it.

## Why

A type is as large as the lake's owner makes it, and the fold runs under
the writer. Kept records made the store hold a type twice at the peak of
one declaration; a step is the size of its event.

A step must answer for one record the same whenever it is asked: the
walk asks at the flush, a later event asks when it reads. So nothing a
step reads may move after its event, which is why it holds its own
declarations and settings and reads no output recorded after it. The
states a walk puts and the states a later read builds on are then one
computation, and a rebuild, which folds many appends as one page, leaves
what the appends left.

Walking at the flush, not at each event, keeps the time what it was: k
recomputations a record for k declarations of one bundle. Steps stacked
on one another would cost k² over two, since each later walk would
apply every earlier step to every row again; a bundle of functions of
one type, and a rebuild of several appends that each change a function,
are common. Ending the transaction after a flush is what lets a fold of
the log never stack: the next walk reads the records as put.

## What it is not

- Not a bound on the calls a recomputation makes due: a function
  computed outside the lake that is declared again makes one call due
  for every record without a recorded output, and they are held until
  they are made.
- Not a bound on what an engine holds of an open transaction; that is
  the engine's (`docs/seams.md`, `ProjectionStore`).
- Not a transaction a page: a page of the log may be folded in several.
  Nothing a read answers depends on where they end; a position that is
  folded answers as it does on the live lake.
- Not a split of an append. An append that held events reading records
  between two runs of declarations would apply the first run's kept
  steps again in its second flush; no door drafts one.
- Not a way to bring a field that reads a setting or the clock up to
  date. Only computed fields decide what a `declared` computes again: a
  type declared again with a field more that is not computed walks no
  record, and a derived expression that reads a setting stays as of its
  record's last write (`docs/hosting.md`). A host that counted on such a
  declaration to refresh it writes the records; a field of a function is
  computed again by a `recompute` of the function. A type with an
  embedding or a field of a function computed outside the lake is walked
  at every declaration of it, whatever changed (022, extended).
- Not a second computation. The walk, the records the transaction wrote
  and a later read all take a record through the same step.
