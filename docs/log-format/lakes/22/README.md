# A lake a release wrote: format 22

`kept22` is a small lake that the code of 0.22.0, which writes log
format 22, made through its doors on the SQLite engine and kept as that
engine wrote it (`log/`, with `export.json`, and the engine's `index/`).
Its log is the writer's stored form, not a copy's: its first append is
stored whole, and every later append of two or more events is one frame.
Its scenario (positions 0 to 55):
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- two partners, three invoices linked, one of them changed and one
  retired;
- a `person` type whose `name` and `email` are erasable; `crm` ingests
  `p1`, `p2` and `p3` in one append, `web` ingests `p2`, and an author
  notes `p2`;
- an ingest of `crm` made on the occasion `caught-up` (`p7`, `p8`), and
  the service `mailer` upserting a partner on the occasion `live` for
  `human:alice`;
- a second ingest of `crm` that asks for its refused records to be
  quarantined: `p4` and `p6` land, `p5` is refused for a member the type
  does not have and quarantined, and a holder of `ingest` dismisses it;
- a record the host refused, `p9` of the source `erp`, quarantined under
  its run `run-1` and its line 4;
- the setting `vat_rate`, declared by a host, with its first value, a
  period that holds from 2026-07-01 and the correction of that period;
- a `tariff` type and a `reading` type whose field `price` is computed
  at the read by `in_force`, a constraint of `reading` and a trigger on
  it that name `preceding`, and a view `cost` over the readings;
- a `sample` type, a view `hourly` that keeps `first`, `last` and `max`
  per hour (`view_kept.hourly`), and three samples;
- `web` is forgotten.

**The forms format 22 holds and 21 does not**, and where this lake
holds them:

| Form | Held |
|---|---|
| `occasion` in an event's `context` (lakelands#1584) | the `caught-up` ingest and the `live` upsert |
| a `record_quarantined` the host refused, under `(run, line)` as its `id` (lakelands#1596) | `p9` |
| `preceding` in a constraint and in a trigger's `when` (lakelands#1552, lakelands#1604) | `reading` |
| `first` and `last` in a view (lakelands#1603) | `hourly` |
| `in_force` in a field computed at the read (lakelands#1606) | `reading.price` |
| an append written whole in one frame, whose end the feed names (lakelands#1540) | every append of two or more events |

Row grants (lakelands#1588, lakelands#1590) write no line: a grant is
configuration, not an event, so the lake holds none.

Format 22 is frozen at the commit `2df88393` of the development
branch, and 0.22.0 only adds to it. That commit writes this lake and
reads it, but for what was added after it: the host's quarantine of `p9`
(lakelands#1596), the types `tariff`, `reading`, `cost` and `sample`
with the declarations that name `preceding`, `first`, `last` and
`in_force`, the trigger on `reading` and its firing, the setting of
`view_kept.hourly` and the records of those types, and the framing of
each append whole (lakelands#1540), where that commit frames the runs of
events that share a `known_at`. The scenario without those additions,
made through that commit's doors, holds the same events in the same
order as this lake's log without them.

It holds besides the forms of 21: a record's erasable values sealed
together in one `sealed.box` and an erasure by `source_forgotten` with
the source's key destroyed (`web`).

`keys/kept22.keys.json` is the file keyring's file that sealed the
values: `crm` has one key version, `web` is destroyed.

`answers.json` keeps what 0.22.0 answered of it through its doors:
every live record of each type it names (`types`, `sys.setting` among
them, and the readings with the price `in_force` finds at the read), the
link records, the view's counts and the kept extremes of `hourly`, the
members of the set `open`, the walk over the relation from `i1` with the
link's fields, and `p1`, `p2`, `p3`, `p4` and `p7` with their
provenance: `p1`'s name read through `crm`'s key, `p2`'s email erased.

`upgrade::LAKE_22_READ` imports the lake on every engine with that key
file, checks the shape of its log and of its index, and holds each
release's answers to those. The directory is never edited: a later
release that answers otherwise says so in its own row, not here.
