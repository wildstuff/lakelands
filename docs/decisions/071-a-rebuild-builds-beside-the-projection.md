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

On SQLite the scratch is `projection.db.rebuild`, beside
`projection.db` in the projection's directory (the lake's, unless its
location names a `projection_dir`). On PostgreSQL it is a schema of its own, named from the
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
the old file open would go on reading it after the rename, and so would
one that connected to it while the swap ran. Every handle onto the
projection therefore holds a shared lock on the lake's name file, taken
before it connects and kept while it lives. The swap takes that lock
exclusively, so it is refused `swap_busy` while another handle holds
the projection, and a handle that opens during the swap waits for it
and connects to the file put in place. A process of a release before
this one takes no such lock. The swap keeps it out only while it has
the file open, by taking the projection out of its write-ahead journal
(which SQLite allows to the one connection that has it open), so the
old release is stopped before the swap. The lock is the operating
system's file lock, so a lake on a file system that keeps none (some
network and user-space mounts) no longer opens, even to be read.

On PostgreSQL the swap is one transaction: it drops the projection's
tables, moves the scratch's into the lake's schema, replaces the
projection's rows of `meta` with the scratch's, drops the scratch's
schema, and makes the log's tables this build has and the lake lacks
(`frames`, in a lake 0.19.0 made), in the log's tablespace. They are
made there and not when the rebuild opens the lake, because a release
refuses a schema holding a table it does not know: an upgrade that
fails before its swap leaves the lake's schema as the older release
opens it, and the upgrade the open appends after the swap finds every
table the log needs (lakelands#1467). Once the swap has run, the older
release refuses the schema; the way back is to open it again with this
release, which finishes the upgrade, or to restore an export into a new
schema. The log shares the lake's schema, so the tables move and the
schema does not: every reader goes on naming the same schema, and each
statement sees the old projection or the new one, never neither. A
transaction that read some of the old tables before the swap and reads
another after it may read that one new, as it may across a rebuild's
`clear`; only `ranked`, which takes every table it reads before its
snapshot, is kept from that. A layout that drops or renames a table
names the old one among the retired tables, so the swap drops it too.
The swap waits for the locks of transactions reading the old tables,
and readers that begin after it wait behind it, so a long reader
stalls the lake for as long as it reads.

### The store builds beside, and a build ahead outruns the upgrade

Every rebuild the store makes builds beside the projection and swaps:
the writer's open of a projection an older build laid out, an open to
rebuild, and `LakeStore::rebuild`. The first goes on from a scratch
marked built where one fits; the other two fold the whole log. Where the
engine keeps no scratch, the store folds in place, as before.

`lakelands build-ahead` runs the first half of an upgrade early. Under
the new release, beside a lake the old release still holds, it opens the
log and its own scratch only, folds the log into it and marks it built.
It says the room it takes before it starts. Run again, it goes on from
its mark. A fold that met an act erasing values, a source forgotten or
values removed, may hold what the act erased, since it folded those
values before it was told the act. Such a build folds the whole log
again, told the act, as a rebuild is. The writer's open that upgrades
the lake then folds only the events after the mark. An act erasing
values among those events is met the same way: the open folds them,
swaps, and then folds the whole log again to finish the act, as the old
release did when it wrote it. The open says so, since it then takes as
long as an upgrade without the build ahead; a build ahead run again
after the act moves that cost to before the upgrade.

A swap refused `swap_busy`, because another handle has the projection
open, is not the open's failure. A reader left running must not keep a
lake from opening, and the scratch cannot be swapped while it reads. So
the store asks the engine first (`LakeEngine::may_swap`), and where a
swap would be refused it folds in place without folding beside: a
rebuild then costs one fold and no second projection's room, not two.
The open of an upgrade removes the build ahead it cannot swap in, and
says so. A reader that comes between the question and the swap is met
as before: the open removes the scratch, gives the writer back what it
knew before the build, and folds in place.

Nothing removes a build ahead on its own. A writer's open swaps in one
that fits its layout, and the next build ahead removes one the log no
longer holds the mark of. A scratch of another layout whose mark the log
still holds may be the next upgrade, already paid for. Only the operator
knows whether that upgrade is still coming, so only the operator removes
it: `lakelands build-ahead --discard`, after the command has said up
front how much room the build takes. The engine measures that room
without opening the projection (`EngineOpener::room`): the files'
sizes on SQLite; on PostgreSQL the tables' sizes, or, where a session
holds a table for more than a second, the sizes the catalogue last
recorded.

On PostgreSQL the builder holds a session lock while its handle lives.
The server lets a session's locks go only when it sees the connection
close, which can be after the next statement of the process that dropped
it. So a dropped builder gives the lock up itself, and a swap that
follows the drop is not refused `rebuilding`.

### On PostgreSQL a graph is built beside too

A field the lake asks an index for keeps a graph per class of reader
(`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`),
and a graph is the second thing built beside: B built again once a hold
has made it stale, or A at the type's first hold. A thread of the writer
builds it on a connection of its own. It names the next generation in
the graph's row, copies the graph's entries into a table of their own in
one snapshot and indexes them, then, in one short transaction under the
fold's lock, applies the keys every write since touched and swaps the
row to the new table. Until the swap, the class's readers are ranked
exactly, never from a graph that no longer describes the head.

A build that fails, is stopped or is killed leaves the row naming the
generation it read, and the graph answers as it did. The builder tries
again after a pause that doubles with each failure in a row, or when a
commit wakes it. The next writer's open gives up every build a writer
before left half done, in one transaction under the fold's lock, and
drops every graph table no row reads, once no transaction holds the
rows. A superseded generation is dropped once every search that might
read it has ended. A rebuild's scratch builds its graphs at the end of
its fold, and the swap above drops the projection's graph tables and
moves the scratch's in with the rest.

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
  every other handle onto the old file, to let go.
- It does not remove a build ahead it cannot use. A scratch marked by
  another layout's build is kept while the log holds its mark, for that
  build to swap in; removing it is the operator's.
- It does not let an older projection be read. A reader's open of a
  projection of another layout is refused as before; only the scratch's
  handle opens beside one, and it never reads it.
- It is not a second projection the lake answers from. The scratch
  answers only its builder, and nothing reads it but the fold and the
  swap's checks.
- It keeps nothing on the memory engine, which keeps no projection
  apart from its handle. There, opening a scratch, marking one and
  swapping are refused as unbound.
