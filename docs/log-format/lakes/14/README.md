# A lake the release before wrote: format 14

`kept14` is a small lake that v0.13.1 (`08e86fa0`), which writes log format
14, made through its doors and exported (`log/`, `export.json`):
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- three invoices linked, one of them changed and one retired.

`answers.json` keeps what v0.13.1 answered of it through its doors:
every live record of each of its types, the link records and the view's
counts among them, the members of the set, and the walk over the
relation from `i1` with the link's fields.

`upgrade::LAKE_14_READ` imports the lake on every engine, which upgrades
it (`docs/log-format.md` §6), and holds this release's answers to those.
The directory is never edited: a later release that answers otherwise
says so in its own row, not here.
