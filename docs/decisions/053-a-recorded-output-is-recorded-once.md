# 053 — A recorded output is recorded once

## What is decided

A seam's output is in the log once per call and answer. Where an output
the lake holds fills a field again, the `output_recorded` that fills it
names that output by `output_at`, the position of the event that carries
it, and carries no `output` (`docs/log-format.md` §2.5).

- **The writer names the call's latest output.** The store compares the
  answer it records with the latest output the lake holds for the same
  `seam` and `input`, under the writer's lease: where they are the same,
  and the answer is no failure, it writes `output_at`. A new answer, a
  failure, and a settle's second output for a call carry `output`, as
  does a verb's output, which fills no field.
- **The fold resolves it and checks it.** It reads the output from the
  outputs recorded before the event, in its own transaction or in the
  projection, and fills the field from it. The output it finds for the
  call must have been recorded at `output_at`; any other position is a
  damaged log, refused and not folded past. The fold records no output
  for such an event, so the projection's recorded outputs are those of
  the events that carry one, and its answers are what they were.
- **One of the two members.** An `output_recorded` that carries both
  `output` and `output_at`, or neither, is refused when the log is read.
- **Format 14.** The first append that holds an `output_at` carries
  `v` 14 (`docs/log-format.md` §6); a reader below 14 refuses it rather
  than filling a field from nothing. A lake whose fills each carry a new
  output stays at the version it had.
- **Every reader of the log may read the event named.** The event at
  `output_at` fills another record's field, or the same record's before.
  The feed is the log: it withholds no event for its reader's rights,
  and over a network door only a principal that reads every record opens
  it (`docs/decisions/037-the-feed-over-a-network-door.md`); an export
  holds every event (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
  So whoever may read the fill may read the output it names, by one read
  of the log at `output_at`. A consumer whose own filter left that event
  out of its polls reads the position without the filter.

This extends `docs/decisions/002-determinism-is-the-boundary.md` and
`docs/decisions/022-functions-are-declarations.md`.

## Why

An embedding declared over records that share texts made the log grow
with the records, not with what was computed. Measured on the memory
engine with 1 536-dimension vectors: 10 000 records sharing 64 texts
appended 10 000 `output_recorded` events, 309.8 MB, of which 64 outputs
were distinct. Named by position, the same fills take about 8 MB. Where
every text is distinct nothing changes.

A replay must read the output and never call the seam (002). The fold
already answers the latest output recorded for a call from its position
on, so a fill could be resolved by `seam` and `input` alone. The
position is written anyway because it serves readers outside the fold:
a feed consumer gets the vector with one read at `output_at`, without
indexing every output it has seen, and the fold's check makes a log
whose reference points elsewhere a damaged log, not a different answer.

The position cannot go stale. The log is append-only, purge retires
records and removes no event, and a sealed segment never changes, so
the event at `output_at` is there for as long as the reference is.

## What it is not

- It changes no answer: the projection's records and outputs, `get`,
  `find` and `search` read as before, and a rebuild, a reopen and an
  export's import fold the same values.
- It does not change how calls are batched, which never reaches the log
  (§2.5), nor what a drain holds.
- It is not a cache of outputs across lakes or calls: only the same
  `seam` and `input` in the same log is named.
- It does not name an output that is not the call's latest: after a
  settle records a second output for a call, a fill names the second.
