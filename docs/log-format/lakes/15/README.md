# A lake a release wrote: format 15

`kept15` is a small lake that v0.14.0 (`6ca132d1`), which writes log format
15, made through its doors and exported (`log/`, `export.json`):
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- three invoices linked, one of them changed and one retired.

`answers.json` keeps what v0.14.0 answered of it through its doors:
every live record of each of its types, the link records and the view's
counts among them, the members of the set, and the walk over the
relation from `i1` with the link's fields.

`upgrade::LAKE_15_READ` imports the lake on every engine (a later release
upgrades it, `docs/log-format.md` §6) and holds each release's answers to
those.
The directory is never edited: a later release that answers otherwise
says so in its own row, not here.
