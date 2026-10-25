# 072 — A vector is stored once, in `vectors`

## What is decided

**In force** since lakelands#1323 (2026-10-23), for 0.18.0, under the
one layout raise of lakelands#1318; proposed on lakelands#1319
(2026-10-22). The SQLite and PostgreSQL projections keep a vector once,
in `vectors`, and the suite's rows hold it on every engine
(`a_vector_reads_the_elements_written_at_each_position_bit_for_bit`,
`a_backends_vector_reads_its_f32_elements_widened_at_each_position`,
`a_sealed_vector_reads_its_bits_until_its_source_is_forgotten`,
`a_links_vector_reads_through_a_walk_at_each_position`,
`a_restriction_and_its_lifting_keep_a_records_vector`,
`a_rekey_ends_the_old_keys_vector_at_its_position`,
`a_migration_that_renames_a_vector_field_reads_its_elements_under_the_new_name`,
`a_json_array_of_numbers_reads_as_written_beside_a_vector`, and through
the engine seam `a_vector_put_before_a_later_state_reads_as_put`).

### The stored state carries no vector elements

A state the SQLite or PostgreSQL engine stores keeps a vector field's
provenance and, in place of its elements, the position of the row of
`vectors` that holds them. A read joins that row, at the head and as of
any position, and answers the elements the write gave, bit for bit. A
field that holds no vector names no row. Only a field the type's
declaration in force declares a vector is kept apart, a link type's by
its relation's fields and a kept name's by its view's: a value of any
other field stays in its state whatever its shape, a `json` array of
numbers among them.
The store's own names, `sys.` and `sys:` other than the link types,
keep a vector in their state as before: an engine reads them, settings
among them, by statements of its own that join no row, and they are few.

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
holds a vector, as a re-key's state at the new key does. A state a
migration writes names the row in force where it keeps the field's name
and elements, and writes one under the new name where it renames the
field, ending the old name's row: the engine is handed a state, not the
migration, so it shares a row by name and elements alone. A vector the
vector index is handed with no state naming it has a row of its own
(`since` -1), which a state that names the field replaces. One handed
for a field whose row in force a state names holds over it, as it does
on the in-memory engine, whose index has no rows of states: a put of
the same elements leaves the state's row, as the fold's put after the
state does, and a put of others, or a remove, ends the state's row at
its own position, so a read as of that state still finds it, and a put
writes a row of its own. The record's next state writes its row anew.
A state put
at a position before a later state of its record keeps its vectors in
place and writes no row: the row at its position may be one a later
state names, with other elements, and it stays as that state wrote it.

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
is null, and on PostgreSQL through the same. A field the lake asks an
index for is read on PostgreSQL through its graphs instead, tables of
their own that hold the unit vectors of the live heads each class of
reader may see, each with its HNSW index
(`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`);
the elements a ranking scores by are still the rows of `vectors`. A search at an earlier
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
  lakelands#1318, so a lake is rebuilt once. A projection a build
  between the two laid out bears the raised mark with `vectors` still a
  head index: the open tells it apart by that table's shape, reads it as
  the layout before and rebuilds it; the check goes at the release that
  ships both. PostgreSQL's layout is raised again to 7 by 078, when the
  unit vectors move from `vectors` to the graphs, and its first open
  rebuilds each PostgreSQL lake once more; SQLite keeps its layout.

## What it is not

- It is no change of the log. An event holds a vector's elements as
  JSON, widened from `f32` where a backend made them, as
  `docs/log-format.md` §5 states.
- It changes no answer, on any engine: every read answers the same
  elements to the bit, and every ranking the same keys, order and
  scores, as the rows above hold.
- It does not round a caller's doubles to `f32`, nor quantise.
- It does not compute PostgreSQL's `unit` from `data`. That would save
  1.5 to 1.7 KB more a record, but PostgreSQL casts no `bytea` to
  `float4` or to pgvector's `vector`, so an index over `data` needs a
  function of the engine's own to decode the elements, and the
  candidates it finds are not shown to be those `unit`, normalised by
  the one Rust code, finds. A ranking's scores are the cosine over
  `data` either way. `unit` stayed, filled for the rows in force of a
  field the lake asks an index for (lakelands#1323), until layout 7
  moved it to the graphs, each of which holds a key and its unit vector,
  normalised by the same Rust code, for the live heads its class may
  see (078); `vectors` keeps the elements alone.
- It keeps no ranking of an earlier position.
