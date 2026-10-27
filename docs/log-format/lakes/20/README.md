# A lake a release wrote: format 20

`kept20` is a small lake that the code v0.20.0 releases, which writes log
format 20, made through its doors on the SQLite engine and kept as that
engine wrote it (`log/`, with `export.json`). Its log is the writer's
stored form, not a copy's: its first append is stored whole. Its
scenario (positions 0 to 29):
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- two partners, three invoices linked, one of them changed and one
  retired;
- a `person` type whose `name` and `email` are erasable; `crm` ingests
  `p1`, `p2` and `p3` in one append, `web` ingests `p2`, an author notes
  `p2`, and `web` is forgotten.

**The forms format 20 holds and 19 does not**, and where this lake
holds them:

| Form | Held |
|---|---|
| the first append, with `lake_created`, stored whole | the first lines of the log |
| the members an append shares, stored once in a frame | the appends after the first |
| a record's erasable values sealed together in one `sealed.box`, with `sealed.fields` naming them | the ingests of `crm` and `web` |
| the ingest id made under `lakelands:ingest:v2` | every ingest |
| an erasure by `source_forgotten`, with the source's key destroyed | `web` |

A log of 19 holds none of these but the erasure.

`keys/kept20.keys.json` is the file keyring's file that sealed the
values: `crm` has one key version, `web` is destroyed.

`answers.json` keeps what v0.20.0 answered of it through its doors:
every live record of each type it names (`types`), the link records, the
view's counts, the members of the set `open`, the walk over the relation
from `i1` with the link's fields, and `p1`, `p2` and `p3` with their
provenance: `p1`'s name read through `crm`'s key, `p2`'s email erased.

`upgrade::LAKE_20_READ` imports the lake on every engine with that key
file, checks the shape of its log, and holds each release's answers to
those. The directory is never edited: a later release that answers
otherwise says so in its own row, not here.
