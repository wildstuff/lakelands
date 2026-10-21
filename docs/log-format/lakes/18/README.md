# A lake a release wrote: format 18

`kept18` is a small lake that the code v0.17.0 releases, which writes log
format 18, made through its doors and exported
(`log/`, `export.json`), from the scenario of lake 17 (positions 0 to 63),
which holds that of lake 16:
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- three invoices linked, one of them changed and one retired;
- a `person` type whose `name` is erasable; `crm` ingests `p1` with a
  name and `p2` with a city, `web` ingests `p2`'s name, an author notes
  `p2`, and `web` is forgotten (positions 0 to 28), then what 17 adds (29 to 63).

Then what format 18 adds over 17 (positions 64 to 83):
- an `item` type whose authored `title` admits the source `samples`, and
  `samples` ingests `m1` with a title (64, 65);
- a verb `cancel` of `effect: retire` that is `granted` (66);
- `hr` ingests `p7` in the delivery `late` sealed apart, `p8` in `later`
  and `p9` in `kept-apart`, each delivery's values sealed under a key of
  its own composed with `hr`'s (67 to 69); `late` is taken back, which
  names its apart key in the `values_removed` and retires `p7` (70, 71);
  `p9` stays, read through the composed key;
- `erp` ingests `p10` in `batch-9`, taken back by `deliveries` (72 to
  74);
- `feed` ingests `p11` and `p12` with the times it saw them, and what it
  saw before a time is taken back by `seen_before`, which retires `p11`
  (75 to 78);
- `erp` ingests `p13` in `batch-10`; `later` of `hr` is taken back, and
  `batch-10` with `batch-7` (already taken back) in one act by
  `deliveries` (79 to 83).

Every `record_ingested` carries its `delivery`: as its caller named it,
the store's own `d<position>` for the others. Its values are sealed
under each source's key: `keys/kept18.keys.json` is the file keyring's
file that made them, `crm`'s, `hr`'s and `ledger`'s second version in
it, `web` destroyed and `ledger`'s first version gone.

**The forms format 18 holds and 17 does not**, and which of them this
lake holds:

| Form | Held |
|---|---|
| a delivery sealed apart: `sealed` composed with the delivery's apart keys (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`) | 67 to 69 |
| `values_removed`'s `apart`, the apart keys it destroys | 70, 80 |
| `values_removed` by `seen_before` | 77 |
| `values_removed` by `deliveries` | 73, 80, 82 |
| a field's `admits`, and a `record_ingested` writing an authored field of a source it admits (`docs/decisions/069-an-authored-field-may-admit-a-named-source.md`) | 64, 65 |
| a verb's `granted` (`docs/decisions/070-a-verb-may-be-kept-to-the-callers-granted-it.md`) | 66 |
| an `output_recorded` sealed apart | no |

The last needs a function backing, which a lake made with nothing bound
but the file keyring has none of; the vectors of format 18
(`docs/log-format/vectors/18/`) hold it too. The four fixes of 0.17.0 add
no form: they are refusals and rules, which the vectors and the suite
hold.

`answers.json` keeps what v0.17.0 answered of it through its doors:
every live record of each type it names (`types`), the link records,
the view's counts, the parcels, the voucher left, the slips, the setting
and the item among them; the members of the set `open`; the walk over the
relation from `i1` with the link's fields; and `p1`, `p2`, `p5`, `p9`
and `m1` with their provenance: `p1`'s name read through `crm`'s key,
`p2`'s erased, `p5`'s read through `ledger`'s second version, `p9`'s
through the composed key of its delivery. `p4`, `p6`, `p7`, `p8`,
`p10`, `p11` and `p13` are among no live records; `p12` is.

`upgrade::LAKE_18_READ` imports the lake on every engine with that key
file and holds each release's answers to those.
The directory is never edited: a later release that answers otherwise
says so in its own row, not here.
