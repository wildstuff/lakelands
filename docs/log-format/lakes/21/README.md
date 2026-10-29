# A lake a release wrote: format 21

`kept21` is a small lake that the code v0.21.0 releases, which writes log
format 21, made through its doors on the SQLite engine and kept as that
engine wrote it (`log/`, with `export.json`, and the engine's `index/`).
Its log is the writer's stored form, not a copy's: its first append is
stored whole. Its scenario (positions 0 to 36):
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- two partners, three invoices linked, one of them changed and one
  retired;
- a `person` type whose `name` and `email` are erasable; `crm` ingests
  `p1`, `p2` and `p3` in one append, `web` ingests `p2`, and an author
  notes `p2`;
- a second ingest of `crm` that asks for its refused records to be
  quarantined: `p4` and `p6` land, `p5` is refused for a member the type
  does not have and quarantined, and a holder of `ingest` dismisses it;
- the service `mailer` upserts a partner for `human:alice`;
- the setting `vat_rate`, declared by a host, with its first value, a
  period that holds from 2026-07-01 and the correction of that period;
- `web` is forgotten.

**The forms format 21 holds and 20 does not**, and where this lake
holds them:

| Form | Held |
|---|---|
| a record refused by a rule and kept, `record_quarantined`, and its close, `quarantine_closed` (085) | `p5` and its dismissal |
| the principal a write was made for, `on_behalf_of` on the envelope (088) | the partner `mailer` wrote |
| the periods of a dated setting, `periods` on the write of `sys.setting`, added and corrected (090) | `vat_rate` |
| an index line that names `id` and `prev` empty (086) | every line of `index/` |

It holds besides the forms of 20: the first append stored whole, the
members an append shares stored once in a frame, a record's erasable
values sealed together in one `sealed.box` (the quarantined record's
among them), and an erasure by `source_forgotten` with the source's key
destroyed (`web`).

`keys/kept21.keys.json` is the file keyring's file that sealed the
values: `crm` has one key version, `web` is destroyed.

`answers.json` keeps what v0.21.0 answered of it through its doors:
every live record of each type it names (`types`, `sys.setting` among
them, with the one period `vat_rate` holds after its correction), the
link records, the view's counts, the members of the set `open`, the walk
over the relation from `i1` with the link's fields, and `p1` to `p4`
with their provenance: `p1`'s name read through `crm`'s key, `p2`'s email
erased.

`upgrade::LAKE_21_READ` imports the lake on every engine with that key
file, checks the shape of its log and of its index, and holds each
release's answers to those. The directory is never edited: a later
release that answers otherwise says so in its own row, not here.
