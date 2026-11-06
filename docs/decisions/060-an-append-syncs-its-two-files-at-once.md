# 060 — An append syncs its two files at once

## What is decided

### An append writes its two files, then syncs them together

An append of the SQLite engine writes its events' lines to the active
segment, then their index lines and the commit line to the segment's
index, and syncs the two files at once. It answers when both syncs have
returned, so it is durable and whole when it answers, as before. A
handle on the same machine that reads the commit line finds the lines it
names, because they were written first.

At a window of one record an append through the store took 4.8–5.9 ms
with the syncs one after the other and takes 3.2–3.3 ms with them at
once, so a writer that commits record by record writes 34–62 % more rows
a second (lakelands#1037).

### A crash may leave the last commit line ahead of its bytes, and every open checks that group

A crash during the syncs can leave the last commit line on disk while
its group's bytes are short, or are not the bytes written, or while one
of the group's index lines before it is torn: lines past the last
commit line are left as before. No earlier group whose syncs succeeded can be
in that state: its append answered only after both syncs.

So every handle, reader or writer, checks the active segment's last
group as it first reads that segment: for each of its events the
segment holds its bytes and the line feed after them, and the bytes hash
to the `hash` its index line names, as the log format defines the hash;
an index line of the group that is no JSON is the crash's too, while a
line that names another position stays damage. A digest is checked, not a scan for zeros: the log does not trust the
file system to show unsynced bytes as zeros or not at all. A group that
fails was never answered:

- **the writer** cuts the index, then the segment, back to the commit
  line before it, as it takes the lease;
- **any other handle** reads the log as ending there, cuts nothing, and
  checks each group it reads after until one passes, so it reads what
  the writer appends in its place.

A later read does not check: a crash that loses what was written ends
the handle too. The check costs about 14 µs an event of that one group:
about 3.5 ms at the default window of 256 events, 14 ms at 1 000.

What stays damage, refused before either file is cut: a segment shorter
than an earlier group's commit, and a sealed segment that is short, each
of whose groups was answered after its syncs.
As with an index shortened by hand, what the files cannot tell from a
crash reads as a crash's tail: a byte changed by hand in the last
group's lines, and a commit line before the last one made no JSON, as
zeros across it make it. The writer cuts there the last group, or the
last two, though they were answered, where before the read that met them
refused them as damage.

### A failed sync is written again, and a second failure stops the handle

A failed sync is never followed by a write the next open would not
check. A sync that fails may leave its bytes in memory marked as
written, so a later sync does not carry them: the append writes its
group again, both files, from what it holds, and syncs once more, and
answers as written where that sync succeeds. Where it fails again, the
append answers `unsure`, since the commit line was written before either
sync: every handle reads its events, and they may not outlast a crash.
The handle then appends, seals and takes the lease no more, refused
`unsynced`, until the lake is opened again, so the group stays the last
one and the next open checks it. On a running machine that check may
find the bytes in memory and not on disk, so a writer, as it takes the
lease, writes the last group again, that group alone, and syncs both
files. Before, a failed sync of the segment answered `nothing`.

## Why

The index's commit line is the only record of where an append ends. The
segment holds the events' lines and nothing else (`docs/log-format.md`
§4.2), and an append is all or none (§4.1). So the index cannot go
unsynced and be rebuilt from the segment: a crash could take the commit
lines of appends already answered, and rebuilding from whole lines would
commit a part of the group that was in flight. Syncing both files at
once keeps both promises and buys most of what one sync would.

## What it is not

- **Not a single sync of the segment** with the index rebuilt from it
  (above). Measured, it would save a further 0.1–0.3 ms an append at a
  window of one.
- **Not a group boundary in the segment.** That is a log format change.
- **Not `syncfs`.** It flushes the file system and measured slower than
  today at every window.
- **Not a change to what a reader sees on a living machine.** A group is
  readable from its commit line on, as before. The time in which it is
  readable and not yet durable is the two syncs at once, where it was
  the index's sync.
- **Not a rule for PostgreSQL.** Its append is one transaction and one
  WAL flush, with no index. Its projection's commit waits for no flush,
  as SQLite's does not
  (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`).

## How it came to be

- The PostgreSQL projection's commit, named here as considered and not
  taken, was taken in its own record, 061 (lakelands#1129).
