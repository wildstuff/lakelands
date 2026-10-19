# The soak

The conformance suite proves what one call answers, and the load run
(`crates/lakelands-cli/examples/load`) proves every door under
concurrent clients for minutes. Neither keeps one lake alive for a day,
kills its process at a bad moment, opens it again and asks whether
everything acknowledged is still there, once, and whether the
projection is still what the log folds to. The soak does
(lakelands#975): one lake under a small business's day for 24 hours on
an engine, its server killed at random, checked after every kill and
every hour.

## What it drives

`lakelands serve --mcp` serves the open book (`examples/open-book/`)
from a scratch root; the driver
(`crates/lakelands-cli/examples/soak/`) is another process and speaks to
it over REST, as `lakelands-client` does. A simulated day, deterministic
from the seed, runs every `--day-secs` of real time (60 by default, so
24 hours walk 1 440 days):

- **The accounting export** ingests partners and invoices in windows,
  from 20 partners growing over the run; paid invoices come back as an
  export repeats its rows.
- **The bank export** ingests the accounts and their movements.
- **The desk** sets credit limits, raises reminders, sends, escalates and
  resolves them, and proposes a write-off that a reviewer accepts.
- **The reader** reads the views `debtor` and `cash_position`, the late
  invoices, a pattern from a partner to its invoices, as of an earlier
  position and at an earlier valid time; a second reader holds `only`
  one partner and no `restricted`.
- **The feed** is followed across every kill.

**The kills.** At a random moment every `--kill-every` (60–600 s by
default) the server is killed with `SIGKILL`, whatever it is doing.
Once an hour it is stopped cleanly instead. On PostgreSQL, once an hour,
every connection to the lake's database is dropped under the running
server, which must refuse and then recover. Between a kill and the next
start the driver opens the lake itself, as a host's next process would:
it verifies the log, restricts what the day made restricted, and now and
then checkpoints.

## What is a red

The run stops at the first one, keeps its root (and its schema) and
exits 1:

1. an acknowledged write that is not in the lake, or not as written;
2. a log that does not verify;
3. a projection that differs from the fold of its log: once an hour an
   export beside the running server, imported into a fresh lake on the
   same engine and read alike at the head and as of three earlier
   positions;
4. the model's sums: `debtor` and `cash_position` against the driver's
   own fold, the set `overdue` against its filter;
5. the feed: a gap or a repeat in its positions across the kills;
6. a refusal the driver did not ask for;
7. the reader with `only` answered a record outside it, or a restricted
   one.

What it measures it reports each hour and never asserts: each kind of
call's latency (p50 and p99), the server's memory, and the lake's size.
Growth that does not level off is brought to the maintainers, not
decided by a number.

## Running it

```text
cargo build --release -p lakelands-cli --bin lakelands --example soak
target/release/examples/soak --lakelands target/release/lakelands \
    --hours 24 --seed 975 --scratch <dir> --json <dir>/report.json
target/release/examples/soak --lakelands target/release/lakelands \
    --hours 24 --seed 979 --pg-url postgres://…/lakelands_soak [--pg-plain] \
    --scratch <dir> --json <dir>/report.json
```

Every knob is an argument, and nothing is read from the environment.
The PostgreSQL database is the soak's own: it drops every connection to
it once an hour. `--minutes`, `--day-secs` and `--kill-every <min>-<max>`
shorten a run. CI runs the short mode (`crates/lakelands-cli/tests/soak.rs`):
two minutes on SQLite, the server killed every ten to twenty seconds,
the deep check once a minute.

**The report** (`--json`) holds one line an hour (`check`), each
connection drop and its recovery on PostgreSQL (`dropped`,
`recovered`), the end (`end`) and, on a red, the red and the root kept.
An hour's line holds:

| member | |
|---|---|
| `day`, `elapsed_s` | the simulated day and the real seconds |
| `kills`, `stops`, `drops` | the kills, the clean stops and the connection drops so far |
| `records`, `resent`, `restricted` | the acknowledged records, the windows sent again after a kill, the records held |
| `deep` | the head, the positions the import was compared at, how far the log verified |
| `memory` | the server's `rss_kib`, and of it `anon_kib` (its heap and SQLite's page cache) and `file_kib` (the binary, and the projection read through `mmap`) |
| `size_bytes` | the lake: the files under its root on SQLite, its database on PostgreSQL |
| `timings` | per kind of call, `n`, `p50_ms` and `p99_ms` |

## The runs of 0.14.0

Release builds, 24 hours, the defaults above, on two cores each,
2026-10-12 to 13; pull request 1002's driver on both. SQLite on
develop `56ea6cd3`, PostgreSQL on develop `8d2002f7` (lakelands#979's
reconnect in). Neither went red.

| | SQLite, seed 975 | PostgreSQL, seed 979 |
|---|---|---|
| days walked | 1 440 | 1 440 |
| kills of `serve` | 246 | 260 |
| connection drops, recovered | none injected | 23 of 23, the slowest after 513 ms |
| records at the end | 15 002 | 14 888 |
| head, verified through | 37 802 | 37 525 |
| deep checks | 23, each compared at three earlier positions | 23 |
| call p50 first → last / worst p99 (ms) | 7.3 → 8.5 / 29.5 | 13.7 → 15.8 / 117.2 |
| ingest p50 / worst p99 | 40.7 → 40.6 / 344.7 | 41.4 → 43.9 / 564.8 |
| upsert p50 / worst p99 | 8.8 → 8.3 / 34.2 | 21.9 → 14.9 / 78.5 |
| `serve`'s memory, first → last | 24.6 → 104.3 MB | 21.0 → 31.1 MB |

The p50s stay level through the day. A p99 above twice its median
comes in single hours spread over the day (on SQLite ingest's in hours
1, 3, 5, 14, 19 and 21; on PostgreSQL call's in 15 and 20), not as a
trend.

**`serve`'s memory on SQLite follows the lake, within bounds the
location sets.** It grew in every hour, where on PostgreSQL it levelled
off at about 31 MB, because on SQLite the projection's pages live in
`serve`'s own process: SQLite's page cache and the projection file read
through `mmap`. A `serve` four minutes after its start, on a soak lake whose
`projection.db` was 17.8 MB, held of its 42.7 MB 16.9 MB of the binary, 11.5 MB of
`projection.db` mapped, 10 MB of anonymous memory (the page cache) and
2.3 MB of heap. Each part has its ceiling in the location's
`SqliteTuning`: `writer_cache_kb` (64 MiB), `reader_cache_kb` (16 MiB a
reader) and `mmap_mb` (256 MiB), and the journal is checkpointed back
every `wal_autocheckpoint_pages` (20 000 pages, about 80 MiB);
`docs/hosting.md` says the same of `mmap_mb`. On PostgreSQL the same
pages are the database server's, and `serve` keeps only its own state.

A probe soak told the cache from a leak (lakelands#975): `serve`, started
again after each kill, grew from 22.9 to 50.8 MB in its first five starts
under the default tuning; with the root's tuning set to a 1 MiB writer
cache, a 256 KiB reader cache and `mmap_mb: 0`, its next five starts
held 29.3 to 34.4 MB, flat, of which 18.3 MB was the binary, while the
projection grew to 50 MB. A host that wants a smaller process sets those
lower; the reads then go to the disk instead.

`anon_kib`, `file_kib` and `size_bytes` were added to the hourly line
after these runs, so their reports carry `rss_kib` alone.
