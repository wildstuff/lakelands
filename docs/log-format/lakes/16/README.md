# A lake a release wrote: format 16

`kept16` is a small lake that v0.15.0 (`76918852`), which writes log format
16, made through its doors and exported (`log/`, `export.json`), from
the same scenario as lake 15:
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- three invoices linked, one of them changed and one retired;
- what format 16 adds (`docs/decisions/058-a-source-may-be-forgotten.md`):
  a `person` type whose `name` is erasable; `crm` ingests `p1` with a
  name and `p2` with a city, `web` ingests `p2`'s name, an author notes
  `p2`, and `web` is forgotten before the export. Its values are sealed
  under each source's key; `keys/kept16.keys.json` is the file keyring's
  file that made them, `crm`'s key in it and `web`'s destroyed.

`answers.json` keeps what v0.15.0 answered of it through its doors:
every live record of each of its types, the link records and the view's
counts among them, the members of the set, the walk over the relation
from `i1` with the link's fields, and `p1` and `p2` with their
provenance: `p1`'s name read through `crm`'s key, `p2`'s erased.

`upgrade::LAKE_16_READ` imports the lake on every engine with that key
file and holds each release's answers to those.
The directory is never edited: a later release that answers otherwise
says so in its own row, not here.
