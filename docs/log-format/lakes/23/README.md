# A lake a release wrote: format 23

`kept23` is a small lake that the code of 0.23.0, which writes log format
23, made through its doors on the SQLite engine and kept as that engine
wrote it (`log/`, with `export.json`, the engine's `index/`, and the file
keyring's `keys/`). Its log is the writer's stored form, not a copy's.
Its scenario (positions 0 to 25):

- the store's `sys.setting` and `sys.aging_rule`, then a `tick` type
  (`at`, `symbol`, `price`, all a source's), a `box` type keyed by `no`
  holding a threshold (`symbol`, `from`, `hi`), a trigger `alarm` on
  `tick`, a view `daily` over `tick`, and a `person` type keyed by `id`
  whose `name` is erasable and `city` is a source's;
- the setting `series_index.tick` naming the pair `symbol`, `at`, and the
  setting `view_kept.daily` asking for the view's records;
- two bounds, `B1` from 2026-05-31 with `hi` 10 and `B2` from 2026-06-01
  with `hi` 20;
- `feed` ingests `t1`, `t2` and `t3` in one delivery, and `alarm` fires on
  `t2`: its guard `price > in_force(box.hi, box.from <= at, box.symbol =
  symbol)` reads `hi` of the bound in force at the tick's `at`;
- `daily` groups the ticks by `date_bucket(at, 'day', 'Europe/Berlin')`:
  one group, the local day of 2026-06-01 (cut at 2026-05-31T22:00Z), with
  count 3 and sum 32;
- `crm` ingests `p1` and `p2` in one delivery under its first key version,
  and `web` writes `p2`'s `name` in a delivery of its own;
- a thinning of `tick` by `at` before 2026-06-02T00:00Z takes the three
  raw ticks; the view's roll-up stands, stranded;
- `tick` is declared again, with a description: the kept view is ended
  and made again, carrying its stranded group under the copy of the
  time-zone database that cut it;
- part of `crm` is taken back: `p1` is retired, `p2` keeps `web`'s value
  with `crm`'s erased, and the one key version `crm` held is destroyed.

**The forms this lake holds that format 22 does not**, and where:

| Form | Held |
|---|---|
| a kept view grouped by a named term, `group_by` a list of group keys | the `daily` declaration |
| `date_bucket(x, 'day', zone)` in a view's group term (lakelands#1667) | `daily`'s group, a Europe/Berlin day |
| `in_force` in a trigger's guard, read from another type's record (100) | `alarm` firing on `t2` |
| the setting `series_index.<type>` (097) | `series_index.tick` |
| an act of thinning (`values_thinned`, `act_whole`) keeping the folds of the kept views (102) | positions 20 and 21 |
| a stranded group's kept record, marked `sys:stranded`, and its `cut` (102, lakelands#1746) | `daily`'s group |
| an act taking part of a source back (`values_removed`, the retirements, `act_whole`), a key version destroyed (065) | positions 23 to 25 |

**A purge is not held, and cannot be here.** No source this lake wrote can
be purged at its last position: `feed`'s three ticks are thinned away,
`crm`'s records are removed and its one key version destroyed, and `web`'s
one live value is `p2`'s name on a record `crm` made, which refuses a purge
of `web` co-owned (039). A purge that retires nothing appends nothing, so no
`source_purged` was written; holding one would take a further delivery whose
records a purge then retires. The retirement and `act_whole` a purge writes
are held by the removal and the thinning (positions 20 to 25), and
`source_purged` is held by the purge checks on every engine against lakes
they make, not against this stored log.

`keys/kept23.keys.json` is the file keyring's file that sealed the
values: `crm` holds no version, its one destroyed by the removal, and
`web` holds one.

`answers.json` keeps what 0.23.0 answered of it through its doors: every
live record of each type it names with its fields and its stranded mark
(`tick` has none, the thinning took them; two bounds; the view's stranded
group with its roll-up; `p2`), the group's `cut` naming the copy of the
time-zone database that first cut it, `p2` with its provenance and `crm`'s
value read as erased, the head, and the position and kind of every event.
It keeps, too, the reports the thinning and the removal answered: those
are what the acts said at the time, and an import cannot be asked them
again, so they are not held here.

The copy named is the one this release's binary is built with (`2026e`,
the jiff database's). A later release under another copy folds the view
again at a writer's open, and the group's record keeps naming the copy
that first cut it. The check therefore holds that a stranded group's
record names a copy, not which copy it is.

`kept_lake::LAKE_23_READ` imports the lake on every engine with that key
file, verifies it, and holds the answers `answers.json` keeps: the head and
every event's position and kind as imported, and every record as imported
and again after a writer opens the lake — which appends its `lease_taken`,
a position `answers.json` does not keep — and rebuilds it. `answers.json`
holds a record as its type, its key, its fields and its stranded mark, and
whether a copy of the time-zone database cut it. The directory is never
edited: a later release that answers otherwise says so in its own row, not
here.
