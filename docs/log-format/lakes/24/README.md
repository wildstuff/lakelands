# A lake a release wrote: format 24

`kept24` is a small lake that the code of 0.24.0, which writes log format
24, made through its doors on the SQLite engine and kept as that engine
wrote it (`log/`, with `export.json`, the engine's `index/`, and the file
keyring's `keys/`). Its log is the writer's stored form, not a copy's.
Its scenario (positions 0 to 21):

- the store's `sys.setting` and `sys.aging_rule`, then a `person` type
  keyed by `id` whose `name` and `email` are erasable and `city` is a
  source's, an `invoice` type whose `amount` and `customer` are a
  source's and whose `status` (initially `open`) and `paid_date` are
  state-move fields, and a verb `pay` on `invoice` setting both;
- `crm` ingests `p1` and `p2` in one delivery under its first key
  version, and `billing` ingests `i1` and `i2` in another;
- `erp` delivers `i3` and `i4`, each with an undeclared field, and both
  are quarantined (`(erp, q1, 0)` and `(erp, q1, 1)`);
- `pay` is called on `i1`: its `status` moves to `paid` and its
  `paid_date` is set, authored by the verb;
- three erasures: `p1` whole, `p2`'s `email`, and the quarantined row
  `(erp, q1, 0)`, whose quarantine is closed as removed;
- the setting `erase_on_close` is set, and the dismissal of `(erp, q1, 1)`
  closes it and erases its values, written by the store.

**The forms this lake holds that format 23 does not**, and where:

| Form | Held |
|---|---|
| a field declared `state_move`, moved only by a verb (114) | `invoice`'s `status` and `paid_date`, moved by `pay` at positions 13 and 14 |
| `values_erased` of a whole record (115) | position 15, `p1` |
| `values_erased` of one field of a record (115) | position 16, `p2`'s `email` |
| `values_erased` of a quarantined row, its quarantine closed as removed (115) | positions 17 and 18 |
| the setting `erase_on_close`, and a close that erases what it closes (115) | positions 19 to 21 |

**What this lake does not hold, and cannot.** A restore that stops at a
position (`restore --through`, 077) makes a lake from an export and writes
no line of its own: what it leaves is a log that ends earlier, a form every
lake has. A judge (113) answers what the rules say of records and
writes nothing. `extract` (112), the value shape answered on every kind
(111) and the refusal of a struct given by position (110) are what the
doors answer, with no form in the log. No erasure here is shredded and no
key version is destroyed: `crm`'s one key seals values outside each
selection, and a quarantined row's cells are not sealed, so each erasure is
asked for without shredding; a shredded erasure and a destroyed key are
held by the erasure checks on every engine against lakes they make, not
against this stored log.

`keys/kept24.keys.json` is the file keyring's file that sealed the values:
`crm` holds its one version.

`answers.json` keeps what 0.24.0 answered of it through its doors: every
live record of `person` and `invoice` with its fields, `p1`, `p2` and `i1`
with their provenance (`p1`'s three erased values and `p2`'s `email` read
as erased, `i1`'s moved fields authored by `pay`), the head, and the
position and kind of every event.

`kept_lake::LAKE_24_READ` imports the lake on every engine with that key
file, verifies it, and holds the answers `answers.json` keeps: the head and
every event's position and kind as imported, and every record as imported
and again after a writer opens the lake — which appends its `lease_taken`,
a position `answers.json` does not keep — and rebuilds it. The directory is
never edited: a later release that answers otherwise says so in its own
row, not here.
