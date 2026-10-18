# A lake a release wrote: format 17

`kept17` is a small lake that the code v0.16.0 releases, which writes log
format 17, made through its doors at develop `bed017fd` and exported
(`log/`, `export.json`), from the same scenario as lake 16:
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- three invoices linked, one of them changed and one retired;
- a `person` type whose `name` is erasable; `crm` ingests `p1` with a
  name and `p2` with a city, `web` ingests `p2`'s name, an author notes
  `p2`, and `web` is forgotten (positions 0 to 28).

Then what format 17 adds over 16 (positions 29 to 63):
- a `parcel` type whose `state` only its verbs write (`authority:
  verb`, `initial: packed`), and its verb `ship`, called on `P-1`
  (29 to 34);
- the host's setting `vat_rate` declared with its group `operator`, and
  its record of `sys.setting`, stamped with that group (35, 36);
- a `voucher` type whose records only verbs retire (`retired_by:
  verbs`), its verb `redeem` of `effect: retire`, and its call on `V-1`,
  whose `record_retired` it causes, carrying the verb (37 to 42);
- a trigger `slip_made`, whose strict guard reads `created()`, fired by
  `upserted` and `ingested` alone and bound to depth 1; a write of `S-1`
  fires it, naming the kind of change and that it made the record, and a
  host's reaction one deeper naming that firing writes `S-2`, which
  drops it at the trigger's own bound (43 to 48);
- an object set `big` and a verb `void` declared, then both withdrawn
  (49 to 54);
- a proposal of declarations, which keeps its plan (55);
- `erp` ingests `p6` in the delivery `batch-7`, which is taken back: the
  `values_removed` of the delivery and the `record_retired` of `p6`,
  which only it wrote (56 to 58);
- `ledger` seals `p4`'s name under its key's first version; the key
  store gains a second version, and after a writer's reopen `ledger`
  seals `p5`'s under it; what `ledger` wrote before `p5` is taken back,
  which destroys the first version, named in the `values_removed`, and
  retires `p4` (59 to 63).

Every `record_ingested` carries its `delivery`: `batch-7` as its caller
named it, the store's own `d<position>` for the others. Its values are
sealed under each source's key: `keys/kept17.keys.json` is the file
keyring's file that made them, `crm`'s key and `ledger`'s second version
in it, `web` destroyed and `ledger`'s first version gone.

**The forms format 17 holds and 16 does not**, and which of them this
lake holds:

| Form | Held |
|---|---|
| a field's `authority: verb` (lakelands#1130) | 29, written at 34 |
| a `setting` declaration, and `group` on `sys.setting` (lakelands#996) | 35, 36, and `sys.setting` at 2 |
| a `record_ingested`'s `delivery` (lakelands#1153) | every ingest |
| a type's `retired_by`, a verb's `effect: retire`, and the `record_retired` its call causes (lakelands#1148) | 37, 38, 42 |
| a trigger's `when_strict` (lakelands#1215) | 44 |
| a `trigger_fired`'s `change` and `created`, a trigger's `on_changes` and the guard's `created()` (lakelands#1217) | 44, 46 |
| `trigger_dropped` (lakelands#1216) | 48 |
| a `declared` of `declare: withdraw` (lakelands#1219) | 53 (an object set), 54 (a verb) |
| a `write_proposed` of declarations carrying its `plan` (`docs/decisions/062-a-change-plans-before-it-acts.md`) | 55 |
| `values_removed`, by delivery and by time, with the retirements it causes (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`) | 57, 58; 62, 63 |
| a key version destroyed: `values_removed`'s `keys` | 62 |
| a migration's report counting `recomputed`, and `unlive` (`docs/decisions/062-a-change-plans-before-it-acts.md`) | no |
| a function's `risk` above `low`, and the `write_proposed` of a `compute` (`docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`) | no |
| a function's input that states no `type` (lakelands#933) | no |
| a recorded output's `input` digested under the oldest version that sealed the values it reads (lakelands#1153) | no |

This lake proposes no migration; the vectors of format 17
(`docs/log-format/vectors/17/`) hold one as 17 writes it. The last three
need a function backing, which a lake made with nothing bound but the
file keyring has none of; the vectors hold each of them too.

`answers.json` keeps what v0.16.0 answered of it through its doors:
every live record of each type it names (`types`), the link records,
the view's counts, the parcels, the voucher left, the slips and the
setting among them; the members of the set `open`; the walk over the
relation from `i1` with the link's fields; and `p1`, `p2` and `p5` with
their provenance: `p1`'s name read through `crm`'s key, `p2`'s erased,
`p5`'s read through `ledger`'s second version. `p4` and `p6` are among
no live records.

`upgrade::LAKE_17_READ` imports the lake on every engine with that key
file and holds each release's answers to those.
The directory is never edited: a later release that answers otherwise
says so in its own row, not here.
