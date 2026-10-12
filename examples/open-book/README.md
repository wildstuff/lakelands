# The open book of a small business

An ontology bundle in the loader's format (`lakelands_core::bundle`): a
small business's receivables, payables and cash, and the reminders it
sends. It answers one question:

> **Who owes me what, since when, and does the cash last until then?**

It is the model the soak keeps alive for a day (lakelands#975), and a
bundle a host can start from.

## What it holds

| Lane | What |
|---|---|
| `value_types/` | `money` (a decimal kept to the cent, in euros), and the closed lists `partner_role`, `direction`, `invoice_status`, `movement`, `dunning_status` |
| `types/partner.yaml` | a business partner: debtor, creditor or both, with the credit limit the business sets |
| `types/invoice.yaml` | a receivable or a payable in one type (`direction`), valid from the day it was issued; `days_overdue` is computed at each read from the read's day |
| `types/bank_account.yaml` | an account and its last known balance |
| `types/transaction.yaml` | a posted movement, keyed by account, value date, amount and reference, as banks give no id |
| `types/dunning.yaml` | one reminder of one overdue receivable, a record a level, written by the business |
| `types/debtor.yaml`, `types/cash_position.yaml` | views: open receivables by partner, and the cash in euros |
| `verbs/` | `set_credit_limit`; `mark_dunning_sent`, `escalate_dunning`, `resolve_dunning`, and `write_off_dunning`, which is `high` risk and so proposed and accepted |
| `object_sets/overdue.yaml` | the open receivables more than thirty days past due |

Four of the types come from exports the business already has
(`authority: source`): an accounting export for partners and invoices, a
bank export for accounts and movements. A reminder is the business's own
record. A host writes `view_kept.debtor` and `view_kept.cash_position`
when it wants the views kept (`docs/seams.md`, `SystemVocabulary`).

## What it leaves out, and why

- **A reminder is raised by an upsert, not a verb.** A verb acts on a
  record that exists, or reaches outside the lake; a new notice is a new
  record, whose key the host mints.
- **The notice's text** is its heading, computed from its level. A
  drafting step that writes prose is not the store's to run.
- **Foreign currency.** `currency` stays a field; the views total euros
  alone.
- **An IBAN's checksum.** The evaluator has no arithmetic over the
  characters of a text, so an IBAN is a text field; a host that wants
  the check makes it before it ingests.
- **What a user interface reads:** display names, plurals, icons,
  colours, a title field, and per-field rules about which model may read
  a value. Lakelands has no word for them, and the store reads none.
