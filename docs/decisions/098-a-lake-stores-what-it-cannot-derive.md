# 098 — A lake stores what it cannot derive

## What is decided

**Proposed** on lakelands#1652, for 0.23.0, one design with lakelands#1649 and
lakelands#1651, lines 11, 12 and 13 of epic lakelands#1578. It answers parts 1
and 2 of the brief; the road of parts is
`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`. It amends 020, 030,
038, 054, 074, 077, 083 and 097, whose list of what waits for this record it
answers. No code lands with it.

**A breaking change, and no data migration.** The operator decided a green field
on 2026-10-31
(https://github.com/wildstuff/lakelands/issues/1652#issuecomment-6547704529):
0.23.0 starts a new storage format with no past behind it, every profile is made
anew, and a lake or an export of an older format is refused by the refusal that
exists. Nothing is translated. The release notes state the break. By the
operator's word of the same day
(https://github.com/wildstuff/lakelands/issues/1652#issuecomment-6547764883) every
build of the round lands on the new format, and the format's own pull requests
come first.

**Every change of format, stored table and public type this record makes:**

- **Log format 23.** `LOG_FORMAT_VERSION` and `OLDEST_LOG_FORMAT` are both 23. An
  append of two or more events is stored as compressed blocks. A block's lines
  leave out each `hash` but the last and any `id` the event decides. Hash, chain,
  ids and digests are what they were. 099 adds two event kinds to the same format.
- **Stored tables on SQLite; `LAYOUT` rises.** New: `keys`, `view_order`. Changed:
  `records` and `routes` hold an integer key, `records` a `flags` column,
  `applied.id` is bytes. Gone: `sort_index_desc`, `text_index_key`. `sort_index`
  and `text_index` keep their columns and primary keys. A new file sets
  `auto_vacuum = INCREMENTAL` and `secure_delete = ON`. The index file holds a
  line a block.
- **Stored tables on PostgreSQL; `LAYOUT` rises.** New: `ids`, `view_order`.
  Changed: `log` holds a row a block, `records` and `records_head` a `flags`
  column. Gone: `frames`, `log_id`, `sort_index_desc`, `text_index_key`.
- **On every engine:** `sys:kept` keeps heads only, `sys:order.<view>` leaves the
  record tables for `view_order`, and a state keeps its derived fields' provenance
  once.
- **Public types.** `EventBody::LakeUpgraded` and its kind go. The `Flaw` variants
  that name a member before its format go. `Situation::LogFormat` stays; its
  `oldest` is 23. The seam's append takes blocks, and `view_order` adds its reads
  and writes; F10 states their count against the budget of 110 methods.
- **Commands and limits.** `lakelands upgrade` goes; `lakelands build-ahead` stays,
  since 099 stands on it. `limits.seal_after_bytes` counts bytes as restored and
  falls to 64 MiB.
- **A dependency.** `flate2` on its pure Rust backend, with `miniz_oxide`, `adler2`
  and `crc32fast`. Each is licensed inside `deny.toml`'s list (read, not yet run).
- **Promises amended by F2.** `docs/log-format.md` keeps every format from 23 on,
  not from 14. `scripts/gates/vectors-kept.sh` follows. `docs/log-format/lakes/14`
  to `22` and the vectors before 23 go.
- **`verify` places a broken chain in a block,** not on an event. Every event's
  hash and link are still checked.

Every claim below is marked **measured**, **read** (in the code at `48383421`) or
**reckoned**, and no sentence mixes them. The machine was shared with other work,
so candidates ran as A/B pairs, interleaved inside one run, with the load beside
each number; absolute rates are lakelands#1637's to give. PostgreSQL ran on a 16.15
cluster with `fsync=off` and `synchronous_commit=off`, the engine's floor of 18
lowered in a scratch build: its times hold no sync, and 18 was not run. Bytes do
not depend on load.

### A record is mostly its own copies

A tick reaches a lake holding its key in every b-tree and its provenance once for
every derived field, beside a stored hash its own line decides. Canonical form
and SHA-256 are 0.8% of a tick's write and 0.3% of a general one (measured,
below), so no form of an event makes a write faster. What a write pays is the
commit, where random inserts into b-trees are settled. So the set below is not a
new encoding. It stores less.

### A block is an append, compressed

**An append of two or more events is written as blocks**, each of at most 1 000
events or 1 MiB of lines. An append of one event stays one plain line, and so does
the append that makes a lake.

```text
{"block":{"end":E,"events":n,"first":F,"hash":H,"prev":P,"raw":r,"stored":z},"v":23}\n
<z bytes: zlib of the block's lines, its frame line first>\n
```

- **The first line is canonical JSON and holds `v`,** so a release that does not
  read the format finds its number, as 083 and 054 ask. An index entry points at
  that line, not into the compressed bytes. Read: `one_format` judges the lake's
  first and last events; the first line of a lake of 23 is plain and says 23.
  Reckoned until F3 proves it the way `read_by_0_19_0.rs` proved it for 0.19.0: a
  0.22.x binary refuses by `LogFormat`, and never calls the lake damaged. A crash's
  tail is that test's to try.
- **zlib, level 6.** `verify.py` stays on a language's standard library and the
  writer takes no C. zstd is 15 B a tick smaller and level with zlib on a general
  record (measured below), which does not pay for a verifier's dependency.
- **`raw` and `stored` are the block's sizes; `end` is its append's last
  position.** The feed reads `appends` from `end` (083), and recovery drops the
  blocks of an append whose `end` never arrived, so an append stays whole across
  a crash (reckoned from `recover`, F3's to prove).
- **A block holds one format,** so `v` on every line ends; the rest of 083's frame
  rules hold inside a block.

Measured over 200 000 lines of a segment, block by block as the writer framed
them, load 8.1 to 8.8:

| B a line | as stored | zlib 6 | no `hash` | and no derived `id` |
|---|---:|---:|---:|---:|
| tick, 249 events a block | 280.7 | 89.7 | 47.7 | **24.9** |
| general, 32 a block, sealed apart | 532.4 | 329.2 | 282.0 | **256.2** |

Compressing the last column costs 2.89 µs a tick and 8.55 a general event under
the writer, and inflating 0.17 and 1.32. Both are lower than for the second
column, 7.79 and 12.36, because hex is what zlib is worst at. A host that writes
one record a call gains nothing in the log; a fixed dictionary for it stays out.

### A line leaves out what the block decides

- **`hash`.** An event's hash is a function of the event, and each event's `prev`
  is the hash before it. A block keeps its last hash in the header. A changed byte
  changes every hash after it and breaks at the block's end, so nothing the chain
  catches goes uncaught.
- **`id`, where the event decides it.** An ingest's id is SHA-256 over its source,
  type, key, version before, patch and box (`docs/log-format.md` §3), all in the
  event; so is a quarantined record's. Every other id is stored.
- **`position` and `prev`** are derived already (083); the header holds a block's
  first of each.

A block's events restore to the same canonical bytes, `hash` and `id`. Restoring
one block costs one inflate and a hash an event: read, canonical form and SHA-256
take 1.75 µs a tick (measured, load 3 to 4). So a read of one event in a full
block costs about 2 ms (reckoned from those rates), where today it reads a line. A
walk restores each block once and pays that per event. Who feels it is a read that
jumps between blocks: a record's history pays up to 2 ms a version, and a poll
pays it once for a block it starts inside. Reads of one event stay rare, as 083
found; F3 keeps the last block restored in the reader. A fold needs no hash of an
event it folds, so a rebuild can skip restoring them, reckoned and left to F4.

### A segment is sized by what it restores

Read: 074's seal hashes the active segment under the writer, about 2 ms a MiB,
0.51 s at 256 MiB. Its digest is over restored events, so with blocks the same 256
MiB would hold about 10M ticks and the seal would take about 20 s (reckoned, 1.75
µs an event and the inflate). `limits.seal_after_bytes` therefore counts `raw`, and
its default falls to 64 MiB, about 400 000 ticks and 0.7 s (reckoned).

### An index line a block, and a row a block

Read: the SQLite index holds a line of about 100 B an event, with its hash. It
goes to a line a block in the shape it has, so position to block is a search:
reckoned at 0.4 B a tick and 3 B a general event. PostgreSQL's `log` holds a row an
event, with `log_id` and `log_segment` beside, 540 B a tick (measured). It goes to
a row a block in `STORAGE EXTERNAL`, since a block is compressed already, and the
ids it answers an applied id from move to `ids (id, at)`: reckoned at 135 B a tick.
`frames` goes, since a header holds what it held. The memory engine keeps what it
is handed, so it needs the seam's change alone.

### A record's key is stored once, on SQLite

SQLite gains `keys (ty, key, id) WITHOUT ROWID`; `records` and `routes` hold the
integer. A tick's key is `symbol:at:seq`, so today each record lands at a random
place in `records`' primary key, `records_at` and `routes_key`. An id given in
order lands at their right edge. That is the commit row of the layer table.

`sort_index` and `text_index` keep the text key, since their readers need "the key
ascends inside a tie" (041, read) and the order of a walk by key. So integer ids
everywhere stay out.

Projection only, A/B, reads in ms. The set is integer keys, both indexes gone,
`applied` in bytes, provenance once:

| SQLite | µs a record | written | after `VACUUM` | page 1 000 | 100 points | window up | window down | lookup |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| tick 1M, load 3 to 9: today | 156.3 | 1 597.5 | 1 467.5 | 32.38 | 6.41 | 2.42 | 2.33 | 0.65 |
| the set without provenance | 61.2 | 1 124.9 | 1 078.9 | 30.34 | 4.78 | 2.33 | 2.44 | 0.52 |
| **the set** | **59.4** | **744.4** | **698.4** | **12.94** | **2.85** | **2.10** | **2.28** | **0.61** |
| tick 300k, load 9: today | 94.7 | 1 569.5 | 1 439.9 | 2.65 | 1.38 | 2.85 | 2.90 | 0.12 |
| **the set** | **38.1** | **731.3** | **685.6** | **3.90** | **1.55** | **2.76** | **3.24** | **0.13** |
| general 300k, load 8: today | 30.1 | 652.4 | 623.6 | 0.57 | 1.26 | | | |
| **the set** | **27.0** | **537.2** | **516.3** | **0.96** | **1.55** | | | |

The cost is named: on a small lake a read in key order pays one more b-tree, up to
1.7× on a page and 1.2× on points. On the large lake the smaller state wins that
back and more. On general records, keys already ascending, the write gains little.

**PostgreSQL keeps its text keys.** Measured, tick 300k, load 9: integer keys took
9% from the bytes and 3% from the WAL, and the insert took 1.41× the time. General
100k, load 8: −1.5% bytes, 1.65× the WAL, 2.15× the time. The scratch's insert
joins through `keys`, which disfavours it a little; not by that much.

### What each b-tree earns

- **`sort_index_desc` goes.** Measured above: without it a window down reads as a
  window up. Measured on PostgreSQL at 300k, once: a backward walk of the ascending
  index with an incremental sort took 0.49 ms for 1 000 rows, against 1.0 ms for
  today's bitmap over both indexes. It needs one more bound, `s.sort_key <= k`,
  beside today's `OR`.
- **`text_index_key` goes.** Read: only deletes use it, and a delete knows the
  values its record held, so it goes by primary key.
- **`sort_index`'s primary key stays,** though it looks spare: read,
  `unsorted_after_sql`, `holders_in_order_sql` and `unsorted_holding_sql` seek it
  by key. A trial without it was withdrawn.
- **`routes`' four indexes stay.** Measured on the general record: `routes_kind`,
  `routes_ty` and `routes_source` cost 14 B each, `routes_key` 33 of 624. Each is a
  door's filter; 099's check stands on two of them. Without the whole table a lake
  saves 75 B and its feed has no index, as 083 found.
- **`records_since` and `records_at` stay.** Measured: 20 B of 624 for the first,
  and a partial `records_at` for heads saves 1 B. `changed()` reads both.
- **`applied.id` goes to bytes:** 16 B of 624, measured.

### State stays JSON text, and the flags take a column

Measured, A/B, load 8.5 to 9.5, 300 000 ticks (100 000 general records on
PostgreSQL, 300 000 on SQLite, in brackets):

| | state, B | field test, µs a row | count of a type, µs a row |
|---|---:|---:|---:|
| SQLite, text | 626 (298) | 1.25 (0.70) | 1.29 (0.73) |
| SQLite, `jsonb` | 538 (258) | 0.20 (0.16) | 0.25 (0.20) |
| SQLite, text and a flags column | 626 (298) | | 0.14 (0.10) |
| SQLite, text, tested in Rust | 626 (298) | 3.45 (1.95) | |
| SQLite, zlib a row, tested in Rust | 246 (131) | 11.76 (9.48) | |
| PostgreSQL, text cast | 627 (301) | 2.20 (1.07) | 4.25 (2.05) |
| PostgreSQL, a `jsonb` column | 792 (368) | 0.17 (0.20) | 0.13 (0.15) |
| PostgreSQL, text and a flags column | 627 (301) | | 0.055 (0.076) |

- **`jsonb` stays out.** On SQLite it saves 88 B and a read must turn it back: a
  page of 1 000 took 1.21 ms by `json()` against 1.42 ms over text with the same
  two tests, against 0.20 ms with the flags in a column. On PostgreSQL it is larger
  than the text. Two engines would keep two forms.
- **A compressed state stays out.** It is the largest gain left, 380 B a tick, and
  a page reads six times slower, 8.4 ms against 1.42, and every filter below would
  leave the SQL. On PostgreSQL it could not be tested in SQL at all.
- **`flags` holds `retired` and `restricted`,** on every version: on SQLite in
  `records`, on PostgreSQL in `records` and `records_head`, since a count at an
  earlier position walks `records` (read). The fold writes it from the state it
  writes. The state keeps its members; what reads it changes nowhere else.

### Every test of a state in SQL goes one way

Read at `48383421`; the column "If state left JSON" is what a later record would
face.

| Test | Under this record | Cost to its read | If state left JSON |
|---|---|---|---|
| `count_in_force`, two tests. SQLite `view.rs:129-130`; PostgreSQL `view.rs:145-146` | the `flags` column | measured: 1.29 to 0.14 µs a row on SQLite, 4.25 to 0.055 on PostgreSQL | as here |
| A vector read's `HELD`, `RETIRED`. PostgreSQL `vector_index.rs:108,110` | the `flags` column | reckoned: a cast's 2 µs a row saved | as here |
| Narrow field filter of `records_after_sql`. SQLite and PostgreSQL `view.rs:112` | stays, JSON in SQL | none | Rust: measured 2.7× on SQLite, 3.45 against 1.25 µs; 9.4× compressed; on PostgreSQL the row travels too (reckoned) |
| Exact-text fill. SQLite `projection.rs:1503-04`; PostgreSQL `projection.rs:946-947` | stays, in SQL, one part at a time (099) | none | Rust, measured 3.45 µs a row |
| Sweep of unused bases. SQLite `projection.rs:1430-31`; PostgreSQL `projection.rs:2135` | stays, as a part (099) | none | a `base` column |
| Proposals: index and list. SQLite `projection.rs:79`, `view.rs:1047-49`; PostgreSQL `projection.rs:235`, `view.rs:1220-22` | stays | none; a table of few rows | columns, no read cost |
| Quarantine: index and list. SQLite `projection.rs:87`, `view.rs:1070-72`; PostgreSQL `projection.rs:241`, `view.rs:1255-57` | stays | none; likewise | columns, no read cost |

Read: a head page without a narrow filter tests no flag in SQL, so its read does
not move. `json_each` over a statement's parameters (SQLite `view.rs:165,172`)
reads no state.

### A kept view keeps its head, and its order apart

Measured, SQLite, 100 000 ticks with three roll-ups, A/B, load 3 to 4:

| kept storage | µs a tick | written | after `VACUUM` |
|---|---:|---:|---:|
| today: every version, order entries as records | 1 123.8 | 11 497 | 10 803 |
| `sys:kept` keeps heads | 847.8 | 9 080 | 8 329 |
| and `view_order`, a compact table | 152.0 | 2 061 | 1 870 |
| no order entries, for scale | 58.1 | 1 538 | 1 415 |

- **`sys:kept` keeps the head.** Read: an earlier read of a kept view computes
  already, and `describe`'s three earlier-position reads are what change with it.
- **`view_order` holds an entry as bytes:** extreme, group, sort key and the
  member's id, and no copy of a value. Read: a state of an order entry holds nothing
  a member's record does not. It is the largest gain measured here, 7.4× in time and
  5.8× in bytes, and it is what candles run on.
- **An extreme over a sorted field** could read its next holder from the source's
  order index and keep no entry. Reckoned; it waits for record 097's `series_index`.

### A state keeps provenance once

Read: of a tick's 619 B of state about 400 are `meta` entries, one for each derived
field, each saying what the version says. A state keeps one entry where they agree.
Measured above: 380 B a tick on a type with derived fields, the page 2.3× shorter
at 1M, nothing on a general record (493.3 B either way). lakelands#1636 takes
derived fields out of the tick, so this shrinks then, and it is first to cut.

### What is set at creation is set at creation

Read: neither `auto_vacuum`, `page_size` nor `secure_delete` is set anywhere. A
green field makes them free; each takes effect only on a new file or a full
`VACUUM`.

- **`auto_vacuum = INCREMENTAL`:** +1.9 B a record, 0.13%. `incremental_vacuum` of
  10 MiB took at most 15.6 ms a turn, and 0.08 s to take 150 MB to 125 MB after a
  third was deleted (measured). 099's reclaim stands on it.
- **`secure_delete = ON`:** a rewrite took 6 to 8 µs a row against 5.5 to 16,
  no measurable difference. A value goes from its page at the delete.
- **`page_size` stays 4 096.** Measured, unpaired, on the general record: 8 192
  held 604.5 B against 623.6 today, and 516.6 against 518.7 on the set.

### The layers

µs a record, unpaired, so read them as shares. The sync is this machine's
`F_FULLFSYNC`, about 6 ms, shared by the 32 or 1 000 records of an append.

| | SQLite, general | SQLite, tick | PostgreSQL, general | PostgreSQL, tick |
|---|---:|---:|---:|---:|
| whole write | 1 143.4 | 279.6 | 1 325.4 | 202.0 |
| event's value | 1.47 | 1.12 | | 1.25 |
| canonical form | 3.20 | 1.90 | 3.17 | 1.96 |
| SHA-256 | 0.40 | 0.25 | 0.43 | 0.27 |
| frame and stored line | 7.26 | 4.85 | 7.72 | 5.23 |
| engine's append | 458.42 | 73.50 | 86.43 | 29.65 |
| of it, sync | 449.34 | 72.11 | off | off |
| fold | 52.87 | 176.48 | 135.31 | 135.53 |
| of it, serialising state | 4.74 | 3.50 | | |
| of it, `records` | 6.31 | 7.04 | | |
| of it, the record before | 0.70 | 1.72 | | |
| of it, indexes | 0.15 | 7.54 | | |
| of it, `routes` | 4.88 | 6.56 | | |
| of it, commit | 22.40 | 143.87 | 118.42 | 128.72 |
| stamping | 9.06 | 4.30 | 9.72 | 4.60 |
| sealing a box | 2.68 | | 3.03 | |

Where a tick's bytes go, measured. SQLite, 1 840 B: log 271; index 101; projection
1 468, of it `records` 729, `sort_index_desc` 129, `text_index`, `text_index_key`,
`sort_index_asc`, `sort_index` 72 each, `records`' key 65, `records_at` 62,
`routes_key` 59, `routes` 58, `applied` 46, `records_since` 19, `routes_ty`,
`routes_source`, `routes_kind` 15 each. PostgreSQL, 2 870 B: `records` 745 and
its key 119, `records_since` 106, `records_head` 122 and its key 105, `log` 431,
`log_id` 54, `log_segment` 32, its key 23, `text_index` 93 and its two indexes 288,
`sort_index` 93 and its three 352, `routes` 93 and its four 213.

### The recommended set, and what stays out

Each candidate was tried, and the last column is the recommendation with its reason.
Bytes of a tick's line unless said; compared with 280.7 as stored.

| Candidate | Measured | Taken |
|---|---|---|
| CBOR | 245.3 | no: zlib alone gives 89.7 |
| CBOR, binary hash, id, box | 196.3 | no |
| and a dictionary of names, types, keys | 164.0 | no |
| and typed instants | 144.9; 3.7 µs to restore against 1.75 | no: more than twice the restore, and a verifier needs a codec |
| A dictionary, compressed a line at a time | 109.0 | no |
| zstd an append, level 1 and 3 | 73.6, 78.7 | no: 15 B, and a dependency for every verifier |
| zstd over the binary form | 78.6 to 79.7 | no: binary adds nothing once compressed |
| Compression over 1 MiB, not an append | 80.9, binary 76.1 | no: no gain over a block, and appends lost |
| zlib an append, level 1, 6, 9 | 98.9, 88.7, 86.7; 1.40, 4.03, 10.23 µs | **level 6** |
| Leaving out `hash`, and a derived `id` | 47.7, 24.9 | **yes** |
| What a binary form's hash is over | the restored canonical bytes, as above | 020 stays: a hash over binary bytes would tie every verifier to the codec |
| `jsonb` state | above | no |
| Compressed state | above | no |
| Integer ids in every table | 1 047 B at 1M, the tie rule lost | no |
| Integer ids in `records` and `routes` | above | **on SQLite** |
| `WITHOUT ROWID` on `records` | 47.4 µs against 46.7, 1 057 B against 1 075, page 6.21 ms against 4.82 at 300k, load 8 | no: nothing with ids in, and reads slower |
| Partial index for heads | 1 B of 1 468 | no |
| `records_since` gone | 17 B of 1 468 | no: `changed()` reads it |
| A descending index gone | above | **yes** |
| `text_index_key` gone | above | **yes** |
| `sort_index`'s primary key gone | a tie needs it | no |
| Page size 8 192 | above | no |
| `auto_vacuum`, `secure_delete` | above | **yes** |
| Older-format code removed | below | **yes**, the operator's |

### The whole

Reckoned, as the sum of the measured parts:

| B a record | today | this record |
|---|---:|---:|
| tick | 1 468 + 271 + 101 = 1 840 | 698 + 25 + 0.4 ≈ **723** |
| general | 624 + 532 + 97 = 1 253 | 516 + 256 + 3 ≈ **775** |
| tick with three roll-ups, projection | 10 803 | **1 100 to 1 900** |

And in time, on a tick: the projection's write is measured 2.6× at 1M. Blocks
add 2.89 µs a tick under the writer. What the whole gives a write through every
layer is lakelands#1637's to measure when F12 is in.

### The series form is a design without code

One log event a delivery holding a compressed column block, one projection row a
block with its range, count, minimum, maximum, first, last and sum.

- **It gains:** about 6 B a tick for 4.7 columns (reckoned), against 723. A window
  reads blocks.
- **It gives up.** Versions per record: a late correction rewrites a block in a new
  event. Triggers: one fires on a block, which changes lakelands#1640. Kept roll-ups:
  one folds a block, which changes lakelands#1634 and lakelands#1636. Grants: a mask
  reaches a column, not a record. `as_of`: a late write moves a block's position. A
  forget or a removal takes a block apart.
- **Its go or no-go** is the first open choice below. Until it is taken lakelands#1639
  is not designed.

### What this changes in record 097

- **All its steps wait.** No build starts on the old layout. Steps 1 to 10 land after
  F12. `sort_index` and `text_index` keep their columns and primary keys, so its
  rows, roads and numbers stand. Only its window down on PostgreSQL gains the bound
  named above.
- **`chosen_through` and its twin go.** 097 keeps no mark for an older writer. Read:
  a 0.22.x writer cannot reach a lake of 23, since its open refuses the format, and
  these two exist only to guard an older binary.
- **Its number fill and series build run on 099's road,** so step 8 lands after R7,
  and `choose_order`'s refusal goes there.

### Older code, and what it gives back

Lines at `48383421`, read and counted; F2 measures them.

| What goes | Product | Specification |
|---|---:|---:|
| `core/log.rs`: `*_FROM`, `converted`, `FormatSteps`, `dated_setting`, older branches of `misplaced` and of a frame | 266 | 150 |
| `error`: the flaws these raised | 35 | |
| `store/log.rs`: `read_as_is` and its parts; export's and `lake.rs`'s format steps | 75 | 60 |
| `lake/open.rs`: upgrading; `lake_upgraded` at its sites | 62 | |
| `sealing.rs`: a value sealed per member | 40 | 60 |
| `fold.rs`: rules by format | 5 | |
| PostgreSQL: `convert`, `HEX_COLUMNS` | 86 | 310 |
| `chosen_through`, both engines | 30 | |
| `conformance/checks/upgrade.rs`, all but the layout's rebuild | | 568 |
| `tests/log_vectors.rs`, what goes format by format | | 900 |
| `tests/read_by_0_19_0.rs`, kept for 0.22 in less | | 150 |
| other checks and tests | | 300 |
| **In the core's budgets** | **599** | **2 498** |

Outside them: about 190 lines of `lakelands upgrade` in the CLI, 2.2 MB of kept lakes
and vectors, and 19 conformance rows. `docs/log-format/vectors/22/verify.py`, 1 139
lines, becomes format 23's: it loses its older forms and gains the block. The
refusal stays: `one_format`, `reads_format`, `Situation::LogFormat`. An older
layout's rebuild stays, since `LAYOUT` will rise again.

### The order of the work

Format first, then 099's road beside record 097's steps. F1 stands alone and helps
today's lake. F2 sets the number; nothing ships until F12, so no release holds half
a format. Each step is one pull request of at most 600 lines at ×1.35; F2 is one
reviewable cut.

| Step | What | Product | Spec. |
|---|---|---:|---:|
| F1 | `routes`' planner pick (099) | 10 | 40 |
| F2 | The green field: format 23, refusal only, gate, vectors, the promise | 30 | 60 |
| F3 | The block: write, restore, ends, tail, digest, `verify`, `verify.py`, `flate2` | 240 | 190 |
| F4 | `hash` and `id` derived; export, import, backup; the seal by `raw` | 130 | 150 |
| F5 | SQLite: an index line a block | 150 | 100 |
| F6 | PostgreSQL: a row a block, `ids`; memory | 220 | 120 |
| F7 | SQLite: `keys`, `flags`, `applied` in bytes, two indexes gone, settings at creation | 280 | 160 |
| F8 | PostgreSQL: `flags`, two indexes gone, the descending bound | 140 | 100 |
| F9 | `sys:kept` keeps heads | 150 | 150 |
| F10 | `view_order`: seam and SQLite | 200 | 170 |
| F11 | `view_order`: PostgreSQL, memory | 130 | 100 |
| F12 | Provenance once | 120 | 120 |
| | **Sum** | **1 800** | **1 460** |
| | **×1.35** | **2 430** | **1 971** |

Removed by F2: 599 product and 2 498 specification. 099 sums this record's estimate
and its own into the raise.

### Conformance rows

- `BLOCK_RESTORES_EVERY_EVENT`: a block's events restore to the bytes, hash and id the
  store made.
- `BLOCK_CHANGED_IS_A_BREAK`: a changed byte anywhere in a block fails `verify`, in
  that block.
- `APPEND_OF_ONE_STAYS_A_LINE`: and the append that makes a lake is plain.
- `BLOCK_DIGEST_OVER_RESTORED_EVENTS`: a segment's digest is what the same events
  without blocks digest to.
- `FEED_APPENDS_FROM_BLOCKS`: an append larger than a block still reads as one.
- `APPEND_TORN_BETWEEN_BLOCKS`: a crash between two blocks of an append leaves none of
  it.
- `EXPORT_IMPORT_KEEPS_APPENDS`: a copy imports and exports again the same files.
- `OLDER_FORMAT_REFUSED`: a lake and an export of format 22 are refused, naming the
  format, and nothing of them is read.
- `SEAL_SIZED_BY_RESTORED_BYTES`: a segment seals at its restored size.
- `FLAGS_FOLLOW_THE_STATE`: a retire, a restriction and their undoing move the count
  of a type, at the head and at an earlier position.
- `KEY_ORDER_THROUGH_KEYS`: a page in key order and a tie in a window answer as they
  did.
- `DESCENDING_WITHOUT_ITS_INDEX`: a window down answers as a window up reversed, ties
  ascending.
- `KEPT_HEAD_ONLY_EARLIER_READ`: a kept view at an earlier position computes what the
  read would.
- `ORDER_ENTRIES_APART`: an extreme's next holder after a delete and a retire.
- `PROVENANCE_ONCE`: a field's provenance reads as it did, where it agrees with the
  version and where it does not.

One test beside the suite, SQLite: a removed value is in no page of the file after
the reclaim. One beside the suite on the log: a binary of 0.22.x refuses a lake of
23 by its format.

### Open for the operator

**The series form: no-go recommended.** By the operator's target, candles and
selected series, a lake's rows stay within what the record form carries, and what
that target pays for is the roll-up, which this record cuts by six to ten. With 723
B a tick (reckoned, above), 6 B a tick in a block (reckoned), 252 trading days:

| Carried | Rows a year | As records | In blocks |
|---|---:|---:|---:|
| daily candles, 5 000 symbols | 1.26M | 0.9 GB | no gain |
| 1-minute candles, 50 symbols | 4.9M | 3.5 GB | no gain |
| 1-minute candles, 5 000 symbols | 491M | 355 GB | 3 GB |
| ticks of 50 selected series, 100 000 a day each | 1.26G | 911 GB | 8 GB |
| a whole market in ticks, outside the target | about 25G | 18 TB | 150 GB |

It flips above about 1G rows a year, or for a series nobody corrects and no trigger
reads. A no-go leaves lakelands#1640, lakelands#1634, lakelands#1636 and lakelands#1639
on records. lakelands#1637's tick numbers are the check: if its write rate at these
rows falls short of the delivery, the answer changes for ticks and stands for
candles.

## Why

- **Time is in sync and b-trees, not in forms.** A binary event gains no time, and
  once compressed it gains no bytes.
- **A format with a past can be cut only through converters.** With none behind it,
  the log can hold only what it must and every reader of an older line goes.
- **A verifier stays on a standard library.** A lake must verify from its directory
  and a document (030). zlib is in every language's library, and a binary form or
  zstd is not.
- **A stored hash checks only its own line,** and the chain does that at a block's
  end. Over a block, hex is the one thing that does not compress.
- **The operator's target is the roll-up.** The largest gain measured is order
  entries apart, not a form of the record.
- **SQL that reads a state stays cheap where it is rare.** Only the count walks a
  type, so only the count gets a column.

## What it is not

- **Not a change of what is hashed.** 020's hash, chain and ids hold over the same
  canonical bytes.
- **Not a new query language, and no state outside JSON.**
- **Not a column store.** That is the series form, left open above.
- **Not a compression of the engines' tables.** Each table stays what SQL reads.
- **Not a promise of these numbers.** They come from scratch builds; the steps measure
  their own, and the suite holds the behaviour.
