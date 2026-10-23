# 072 — A vector is stored once, in `vectors`

## What is decided

**Proposed** on lakelands#1319 (2026-10-22), for 0.18.0; it comes in
force when lakelands#1323 builds it, under the one layout raise of
lakelands#1318. Until then the SQLite and PostgreSQL projections keep a
vector twice, as JSON in the stored state and as `f32`s in `vectors`,
and the suite's rows hold what both layouts answer
(`a_vector_reads_the_elements_written_at_each_position_bit_for_bit`,
`a_backends_vector_reads_its_f32_elements_widened_at_each_position`,
`a_sealed_vector_reads_its_bits_until_its_source_is_forgotten`,
`a_links_vector_reads_through_a_walk_at_each_position`).

### The stored state carries no vector elements

A state the SQLite or PostgreSQL engine stores keeps a vector field's
provenance and, in place of its elements, the position of the row of
`vectors` that holds them. A read joins that row, at the head and as of
any position, and answers the elements the write gave, bit for bit. A
field that holds no vector names no row.

### A row is keyed by the state that wrote it

`vectors` holds one row per `(ty, field, key, since)`, where `since` is
the position of the state that wrote those elements. A state that
changes the record's other fields, or none of them (a retirement, a
restriction and its lifting), names the row its predecessor named and
writes none; a state that writes other elements writes a row of its
own; a state that clears the field names none. So a read as of a
position finds the elements of that position by one lookup of the key
its state names, never by searching the rows' intervals. A row carries
`until`, the position from which it is no longer the head's: the first
state that names another row or none, or the record's retirement, whose
state still names it. A re-key ends the old key's row at its position,
as it ends the old key's state. A record written again after its
retirement starts from no fields, so its state writes a row anew if it
holds a vector, as a re-key's state at the new key does and a state a
migration writes, under the field's name in force.

### Elements are doubles, or `f32`s where every one is exactly an `f32`

A row holds its elements as 8-byte doubles, or as 4-byte `f32`s with a
flag that says so where every element is exactly an `f32`, as every
element an embedding backend makes is. A read widens the `f32`s, so it
answers the double the write gave in both forms, in the log's number
form as today (`docs/log-format.md` §1.2): an integral element as an
integer, `-0` as `0`. A ranking takes each
element as an `f32` in both, as it does today, so the cosine
(`025 § A vector ranking is the cosine, best first, exact to the bit`)
scores to the same bits.

### The head's rows are what a ranking reads, through partial indexes

A ranking reads the rows whose `until` is empty, which are the head's:
on SQLite through a partial index over `(ty, field, key)` where `until`
is null, on PostgreSQL through the same, and the HNSW index of a field
the lake asks an index for is partial on it too. A search at an earlier
position goes on ranking by the head
(`025 § A search at an earlier position ranks by the head`).

### A forgetting erases the rows with the states

A forgetting of a source that wrote a type keeping vectors restates the
lake by the rebuild, never the scoped refold, as it does now
(`058 § The projection holds plaintext, and the act restates it`). The
rebuild clears `vectors` with every other table and folds the log again,
and a value erased writes no row, so no erased vector is readable at
any position, and the reclaim gives the rows' pages back.

### The memory engine keeps its vectors as it does

The memory engine stores nothing past its handle and its layout is not
raised. Its states keep the elements, and its index the head's, as now;
it answers what the other engines answer by the same rows.

### These reads join `vectors`

`ProjectionView::get`, `ProjectionView::get_many` and
`ProjectionView::scan` join it, at the head and as of a position, and
the fold reads a record's earlier state through them.
`ProjectionView::edges` joins it too, at the head and as of a position:
a relation carried by link records may declare a vector field, and an
edge answers its link's fields, elements included. `LakeEngine::ranked`
reads its head rows, as it reads `vectors` today. `ProjectionView::count`,
`matching` and `of_source` read no element, and the feed and an export
read the log. The rights and paging of these readers are not
settled here.

## Why

The 0.18.0 review measured a vector's cost at 100 000 records
(lakelands#1295). On SQLite the state's copy is +4 758 B a record and
`vectors` +2 053; on PostgreSQL +3 717 B after TOAST and +1 680. A
backend's `f32`s are written widened to doubles, whose shortest decimal
form runs to 17 digits, so their state copy is about 8 KB. With a
vector index, PostgreSQL holds one vector five times: log, state,
`data`, `unit` and HNSW. The state copy is the largest of them and the
only one no read needs: the log is the truth (001), `vectors` already
holds the elements a ranking reads, and a read can join it. Dropping it
saves about 3.2 KB a record for vectors a caller writes and about 8 KB
for a backend's on SQLite, and about 2.2 KB on PostgreSQL.

Keying a row by the state that wrote it keeps the history exact without
copying a vector into every later state, and a record whose other
fields change, the common write, writes no vector. Keeping doubles
where a caller wrote doubles keeps every answer as it is; keeping
`f32`s where they are exact halves the row for every vector a backend
makes, with no element changed.

Three risks are taken with it:

- **Every read of a record with a vector touches a second table.** A
  `get` or a `scan` of a type with vector fields makes one lookup a
  field and record, by key. A read that needs no vector pays it all the
  same.
- **A history read must find the vector at its position.** A state that
  names no row, or a row the wrong state names, answers another
  vector or none as of that position. The row of
  `a_vector_reads_the_elements_written_at_each_position_bit_for_bit`
  holds every position of records whose vector and other fields both
  change, whose vector is cleared, and that are written again after
  their retirement; `a_links_vector_reads_through_a_walk_at_each_position`
  holds a link's through a walk.
- **The layout is raised, and every lake is rebuilt at the upgrade.**
  The first open of the release rebuilds each SQLite and PostgreSQL
  lake from its log (`004 § The projection keeps an index of the log, and the log answers`),
  beside the projection it replaces
  (`071 § The projection is never emptied for a rebuild`), at a time
  that grows with the lake. It is taken in the one raise of
  lakelands#1318, so a lake is rebuilt once.

## What it is not

- It is no change of the log. An event holds a vector's elements as
  JSON, widened from `f32` where a backend made them, as
  `docs/log-format.md` §5 states.
- It changes no answer, on any engine: every read answers the same
  elements to the bit, and every ranking the same keys, order and
  scores, as the rows above hold.
- It does not round a caller's doubles to `f32`, nor quantise.
- It does not compute PostgreSQL's `unit` from `data`. That saves 1.5 to
  1.7 KB more a record, and lakelands#1323 takes it only if the cosine
  comes out bit for bit the same.
- It keeps no ranking of an earlier position.
