# A lake a release wrote: format 19

`kept19` is a small lake that the code v0.18.0 releases, which writes log
format 19, made through its doors and exported
(`log/`, `export.json`), from the scenario of lake 18 (positions 0 to 83),
which holds that of lake 17:
- a `partner` type and an `invoice` type with a reference to it;
- a relation `approved_by` carried by link records with a field `since`;
- an object set `open` and a view `by_state` that counts invoices by
  their state;
- three invoices linked, one of them changed and one retired;
- a `person` type whose `name` is erasable; `crm` ingests `p1` with a
  name and `p2` with a city, `web` ingests `p2`'s name, an author notes
  `p2`, and `web` is forgotten (positions 0 to 28);
- what 17 adds (29 to 63): a `parcel` type whose `state` only its verb
  `ship` writes, the setting `vat_rate` with its group, a `voucher` type
  whose records only its verb `redeem` retires, a trigger `slip_made`
  and the reaction it drops at its depth, an object set and a verb
  declared and withdrawn, a proposal, a delivery `batch-7` taken back
  and what `ledger` wrote before a time taken back with its first key
  version destroyed;
- what 18 adds (64 to 83): an `item` type whose `title` admits the source
  `samples`, a verb `cancel` kept to a grant, deliveries of `hr` sealed
  apart and two taken back with their apart keys destroyed, a delivery
  of `erp` and several deliveries taken back, and what `feed` saw before
  a time taken back.

Then what format 19 adds over 18 (positions 84 to 89):
- a `meter` type, and `M-1` and `M-2` made on it (84 to 86);
- a verb `read_km` on `meter` of `effect: write`, whose `requires` reads
  the call's payload (`payload.km >= km`), with one precondition named
  `positive` (`payload.km > 0`), and whose `set` writes the payload's
  `km` (87);
- `read_km` called on `M-1` with a payload of `km: 120`, and the write it
  makes (88, 89).

Every `record_ingested` carries its `delivery`: as its caller named it,
the store's own `d<position>` for the others. Its values are sealed
under each source's key: `keys/kept19.keys.json` is the file keyring's
file that made them, `crm`'s, `hr`'s and `ledger`'s second version in
it, `web` destroyed and `ledger`'s first version gone.

**The forms format 19 holds and 18 does not**, and which of them this
lake holds:

| Form | Held |
|---|---|
| a verb's `preconditions`, each a `name` and the condition it `holds` (`docs/decisions/028-authoring-is-strict-and-whole.md`) | 87 |
| a `payload.<name>` in a verb's `requires` or preconditions, which reads the call's payload (`docs/decisions/028-authoring-is-strict-and-whole.md`) | 87, called at 88 |

The format's one step changes no member and no fold: a log of 18 holds
none of these, and nothing else in 19 differs from 18.

`answers.json` keeps what v0.18.0 answered of it through its doors:
every live record of each type it names (`types`), the link records,
the view's counts, the parcels, the voucher left, the slips, the setting,
the item and the meters among them; the members of the set `open`; the
walk over the relation from `i1` with the link's fields; and `p1`, `p2`,
`p5`, `p9`, `m1` and `M-1` with their provenance: `p1`'s name read
through `crm`'s key, `p2`'s erased, `p5`'s read through `ledger`'s second
version, `p9`'s through the composed key of its delivery, `M-1`'s `km`
written by the verb's call. `p4`, `p6`, `p7`, `p8`, `p10`, `p11` and
`p13` are among no live records; `p12` is.

`upgrade::LAKE_19_READ` imports the lake on every engine with that key
file and holds each release's answers to those.
The directory is never edited: a later release that answers otherwise
says so in its own row, not here.
