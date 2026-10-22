# 071 — A rebuild builds beside the projection, and swaps it in checked

## What is decided

Decided on lakelands#1322 by the operator (2026-10-22), for 0.18.0. It
extends 001, whose projection is rebuilt from the log, and 004, whose
open of a projection an older build laid out rebuilds it, by the place
the rebuild folds into. The engines' half lands first; the store's open
and rebuild, and the command that builds ahead, follow on the same issue.

### The projection is never emptied for a rebuild

A rebuild folds into a scratch beside the projection the lake answers
from, never into that projection. Until the swap, every handle reads the
projection as it was. A rebuild that fails, is stopped or is killed
leaves it answering, and the scratch is removed: by its own handle when
it ends, else by the next writer's open of the lake.

On SQLite the scratch is `projection.db.rebuild`, in the lake's
directory. On PostgreSQL it is a schema of its own, named from the
lake's, which holds the projection's tables and reads the log from the
lake's schema. Opening it opens the log and the scratch only: the
projection the lake answers from is not opened, whatever layout made
it, and the lease is not taken. One handle at a time builds a scratch;
a second is refused `rebuilding`.

### A scratch is marked built, and kept while the log holds its mark

The builder marks the scratch built: the lake, the layout it was made
at, the position it is folded through and the log's hash there. A
scratch is kept across handles, and opened again where it stopped, only
while all of these hold: it is this lake's, the log holds that hash at
that position, and the scratch is still folded through it. A scratch
marked at this build's layout must also be laid out by it. Any other
scratch is removed and made anew, empty. A scratch marked by a build of
another layout, whose mark the log holds, is left for that build, so
that one release does not remove what another built ahead.

### The swap is checked, and is one step

A swap takes the writer's lease and puts the scratch in the place of the
projection, but only once it is checked. No handle may have the scratch
open (`rebuilding`), the scratch must be marked built and fit as above
(`no_scratch`), and its mark must be the log's tip (`scratch_behind`).
A refused swap changes nothing. Once it is done, the writer reads and
folds the new projection, and the old one and the scratch are gone.

On SQLite the swap renames the scratch over `projection.db`. Both files
are in one directory, so the rename is atomic. A handle that still has
the old file open would go on reading it after the rename. The swap
therefore takes the old file's exclusive lock first, and is refused
`swap_busy` while another handle reads it. On PostgreSQL the swap is one
transaction: it drops the projection's tables, moves the scratch's into
the lake's schema, replaces the projection's rows of `meta` with the
scratch's, and drops the scratch's schema. The log shares the lake's
schema, so the tables move and the schema does not: every reader goes on
naming the same schema, and sees the old projection or the new one,
never neither.

## Why

A rebuild folds the whole log, so its time grows with the lake. Done in
place, it emptied the projection first. The lake then answered nothing
until the fold reached the log's end, and a rebuild that failed halfway
left no projection at all. An upgrade that changes the projection's
layout rebuilds every lake, so every lake paid that pause and that risk
at once.

Built beside, the pause shrinks to the swap and the fold of the tail
behind the mark. The old projection is the fallback until the new one
is whole. The mark is what lets a scratch outlive its builder. A build
run ahead, under another process, is worth keeping only if it is a fold
of this log: a lake can be restored from an older backup or imported
anew, and a scratch left from before would answer for a log that is no
longer there. A position alone does not
show that the log is the same; the position and its hash together do,
because the chain makes every earlier event part of that hash.

Checking before the swap, and not after, keeps a failed check harmless.
The projection is only ever replaced by one folded to the very event
the writer's log ends at.

## What it is not

- It is not a rebuild without a pause. The swap needs the writer's
  lease, so it waits for the process that holds it, and on SQLite for
  every reader of the old file, to let go.
- It does not let an older projection be read. A reader's open of a
  projection of another layout is refused as before; only the scratch's
  handle opens beside one, and it never reads it.
- It is not a second projection the lake answers from. The scratch
  answers only its builder, and nothing reads it but the fold and the
  swap's checks.
- It keeps nothing on the memory engine, which keeps no projection
  apart from its handle. There, opening a scratch, marking one and
  swapping are refused as unbound.
