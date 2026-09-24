# Walk 1 — the liquidity question

Walk 1 is the definition of done for Lakelands' first phase: from a fresh
clone, a person builds the CLI, creates a lake, applies an ontology,
ingests two months of data, and serves the lake to an agent over MCP. The
agent answers one question, with provenance, and then answers it again as
the lake stood a month earlier.

> **Which debtors are over their credit limit and more than 30 days
> overdue?**

This page is the script. Every command and every tool call is written
down with the answer it must give, and every answer is derived by hand
from the CSV files below. An answer that differs is a **rift**: the walk
records it and goes on; it is never smoothed over on the spot.

## The materials

`examples/liquidity/` is the ontology bundle, in the loader's format
(`lakelands_core::bundle`), with its data under `data/`:

| File | What it holds |
|---|---|
| `value_types/money.yaml` | `money`: a decimal kept at two fractional digits (`scale: 2`) |
| `value_types/invoice_status.yaml` | `invoice_status`: text, one of `open`, `paid`, `cancelled` |
| `types/partner.yaml` | `partner`, keyed by `id`: `name`, `credit_limit` (money) |
| `types/invoice.yaml` | `invoice`, keyed by `number`: `partner` (a reference), `issued_on`, `due_on`, `net`, `gross` (derived: `net * 1.19`), `status`, `paid_on`, `summary` (searchable); the relation `billed_to` via `partner`, and `approved_by` to `partner`, carried by link records with a `since` (`docs/decisions/035-a-link-is-a-record.md`) |
| `types/transaction.yaml` | `transaction`: a payment settling an invoice (not ingested in this walk) |
| `verbs/pay_invoice.yaml` | `pay_invoice`, risk `low`: sets `status` to `paid` and `paid_on` |
| `triggers/invoice_overdue.yaml` | fires when an invoice is open and more than 30 days past due |
| `object_sets/overdue.yaml` | the open invoices more than 30 days past due |
| `data/partners.csv` | four partners and their credit limits, exported on 31 August |
| `data/invoices-august.csv` | the invoice export of 31 August: ten invoices |
| `data/invoices-september.csv` | the export of 30 September: the same invoices, one now paid, two new |
| `data/partners.map.json`, `data/invoices.map.json` | the column maps `lakelands ingest --map` takes |

Two tests keep these from rotting: `crates/lakelands-cli/tests/walk_1.rs`
loads the bundle and runs the commands below on the in-memory mock, and
`crates/lakelands-store/tests/walk_1.rs` asks the question below on the
real store and checks the answers on this page.

**Why three CSV files.** A CSV file holds one type. The question needs
invoices and credit limits at the earlier position, so the partners go
in first and each month's invoices are one ingest of their own. The walk
pins the position of the **August invoice ingest**: that is "the lake as
it stood last month", and the September ingest is what moves the head
past it.

**When the walk can run.** The question uses `now()`, which in a read is
the time of the read, pinned or not. The overdue invoices fell due in
2025 and the ones not yet due fall due in 2030, so the answers below hold
for a walk on any day from 1 November 2025 to 28 February 2030.

## Found before the walk

- **`apply` declared a bundle one declaration at a time.** The loader
  emits types in name order, so `invoice` comes before the `partner` its
  `partner` field refers to, and the store refuses a reference to a type
  not declared yet. `lakelands apply examples/liquidity` therefore failed
  on the real store (it passed on the mock, which did not check). Ruling:
  a bundle is declared whole, through a new door `Catalog::apply` that
  validates the set together and appends it as one append; `lakelands
  apply` calls it, and `declare` keeps refusing a dangling reference.
  Held by `declaring_the_bundle_one_by_one_in_the_loaders_order_is_refused`
  in `lakelands-store`. Resolved: `Catalog::apply` is in, `lakelands
  apply` calls it, and the answers test beside that one applies the
  bundle through it.
- **The bundle declared `money` without a scale.** `money` was a bare
  decimal, so `gross = net × 1.19` kept all four fractional digits
  (`4760.0000`) and so did every sum over it: exactly what that
  declaration means. Read without the bundle open, four digits looked
  like the store's fault. The bundle now declares `scale: 2`, and the
  store keeps every `money` value, derived ones and sums included, at two
  digits; the answers test checks the two-digit form. The lesson belongs
  to the bundle format: a money value type without a `scale` is a bare
  decimal, whatever its name or description says.

## The data, and the answer by hand

Credit limits (`partners.csv`, source `crm`):

| Partner | Name | Credit limit |
|---|---|---|
| P-100 | Harbor Supplies | 10000.00 |
| P-200 | Northwind Tools | 5000.00 |
| P-300 | Cedar Logistics | 8000.00 |
| P-400 | Maple Foods | 20000.00 |

Invoices. `gross` is `net × 1.19`. "Overdue" means open with `due_on` more
than 30 days before the day of the walk; with these dates that is every
open invoice due in 2025 and none due in 2030.

| Invoice | Partner | Due | Net | Gross | August | September | Overdue in August | Overdue in September |
|---|---|---|---|---|---|---|---|---|
| INV-1001 | P-100 | 2025-03-31 | 4000.00 | 4760.00 | open | open | yes | yes |
| INV-1002 | P-100 | 2025-05-01 | 5000.00 | 5950.00 | open | open | yes | yes |
| INV-1003 | P-100 | 2030-01-31 | 1000.00 | 1190.00 | open | open | no | no |
| INV-1004 | P-100 | 2030-02-28 | 1500.00 | 1785.00 | — | open | — | no |
| INV-2001 | P-200 | 2025-06-30 | 3000.00 | 3570.00 | open | open | yes | yes |
| INV-2002 | P-200 | 2025-07-31 | 2000.00 | 2380.00 | open | open | yes | yes |
| INV-2003 | P-200 | 2025-02-28 | 800.00 | 952.00 | paid | paid | no | no |
| INV-3001 | P-300 | 2025-05-31 | 4000.00 | 4760.00 | open | open | yes | yes |
| INV-3002 | P-300 | 2025-06-30 | 3000.00 | 3570.00 | open | **paid** | yes | no |
| INV-4001 | P-400 | 2025-08-31 | 6000.00 | 7140.00 | open | open | yes | yes |
| INV-4002 | P-400 | 2025-01-31 | 2500.00 | 2975.00 | cancelled | cancelled | no | no |
| INV-4003 | P-400 | 2025-09-30 | 3000.00 | 3570.00 | — | open | — | yes |

Overdue gross per partner, against its limit:

| Partner | As of the August ingest | Over? | At the head (after September) | Over? |
|---|---|---|---|---|
| P-100 | 4760.00 + 5950.00 = **10710.00** of 10000.00 | **yes** | 10710.00 of 10000.00 | **yes** |
| P-200 | 3570.00 + 2380.00 = **5950.00** of 5000.00 | **yes** | 5950.00 of 5000.00 | **yes** |
| P-300 | 4760.00 + 3570.00 = **8330.00** of 8000.00 | **yes** | 4760.00 of 8000.00 | no |
| P-400 | **7140.00** of 20000.00 | no | 7140.00 + 3570.00 = 10710.00 of 20000.00 | no |

**The answer at the head:** P-100 Harbor Supplies and P-200 Northwind
Tools. **The answer as of the August ingest:** P-100, P-200 and P-300
Cedar Logistics. P-300 dropped out because INV-3002 was paid in
September.

## The commands

From a fresh clone, in the repository's root. The `walk` directory may
be created anywhere; the page creates it inside the clone, so the paths
to the example are short. The walk runs on the
SQLite engine, the default of `lake new`: a lake kept in memory lives
only as long as its process, so a sequence of CLI commands cannot walk
one.

```sh
cargo install --path crates/lakelands-cli
lakelands init walk
cd walk
lakelands lake new liquidity
lakelands apply ../examples/liquidity
lakelands ingest ../examples/liquidity/data/partners.csv \
  --source crm --map ../examples/liquidity/data/partners.map.json
lakelands ingest ../examples/liquidity/data/invoices-august.csv \
  --source erp --map ../examples/liquidity/data/invoices.map.json
lakelands ingest ../examples/liquidity/data/invoices-september.csv \
  --source erp --map ../examples/liquidity/data/invoices.map.json
lakelands feed --kind trigger_fired
lakelands serve --mcp   # an MCP client starts this itself; see below
```

What each must print:

| Command | Expected |
|---|---|
| `init`, `lake new` | the root and the lake's location, exit 0 |
| `apply` | `applied 8 declaration(s), in force from position 11`, then the eight: value types `invoice_status`, `money`, types `invoice`, `partner`, `transaction`, verb `pay_invoice`, trigger `invoice_overdue`, object set `overdue`. The lake also declares `sys.link.invoice.approved_by`, the link type of `approved_by`, in the same append |
| partners ingest | one `committed` line: **4 records, 0 already in the lake**, through position **16** |
| August ingest | one `committed` line: **10 records, 0 already in the lake**, through position **41**: ten invoices, and for each of the seven that are open and overdue one `trigger_fired` and one `object_set_changed` |
| September ingest | one `committed` line: **3 records, 9 already in the lake**, through position **48**. The nine unchanged rows append nothing. INV-3002 is now paid and leaves `overdue`; INV-1004 is new and not overdue; INV-4003 is new, overdue, fires the trigger and joins `overdue`. |
| `feed --kind trigger_fired` | eight `trigger_fired` events: INV-1001, 1002, 2001, 2002, 3001, 3002, 4001 in August, INV-4003 in September |

The positions are the store's as of `0.1.0`: the lake's first append
(`lake_created`, `lease_taken`, the `sys.setting` declaration), then,
for every command that writes, one `lease_taken` as that process becomes
the lake's writer before its own events. Write down the position the
August ingest prints: it is the one the walk pins. A different number is
not a rift if the records and the answers are right.

## The agent's calls

`serve --mcp` speaks MCP over stdio. After `initialize` and `tools/list`,
the agent makes these `tools/call`s, in order. Every answer carries the
`position` it was read at.

Without a chat product in the loop, the calls are made with the
`mcp_call` example client from the `walk` directory. It starts the
server, makes its requests in one session, and prints each answer's
position line and structured content. The structured content is the
reply's whole envelope, `{ "data": …, "position": N }`; where this page
shows an expected answer, it shows `data`. The client starts
`lakelands serve --mcp` itself, from the `--server` command it is given,
so the server need not be run by hand. A pin holds within one session
only, so each run below makes a pinned turn's calls together.

Every `serve --mcp` process becomes the lake's writer and appends one
`lease_taken`, so each run moves the head by one: it is 49 in the first
run after the September ingest, 50 in the next, and so on. `<head>`
below is the run's own head. A reply's `position` is the position it
reflects; `pin`'s reply also says what is `pinned`.

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" tools/list
```

Among the tools are `invoice_approved_by_link` and
`invoice_approved_by_unlink`, the doors of `approved_by`'s links, and
the `find` and `get` tools of its link type. This walk does not call
them.

**1. `describe`** — `{}`:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" tools/call describe '{}'
```

The lake's types (`invoice`, `partner`,
`transaction`, `sys.setting`, and `sys.link.invoice.approved_by`, the link
type of `approved_by`), the verb `pay_invoice`, and each field
with its authority: `invoice.gross` derived, `invoice.status` shared,
`partner.credit_limit` from a source.

**2. `pin`** — `{}`. Pins the head for the turn: `{ "position": <head>,
"pinned": <head> }`. Calls 2 to 4 are one turn, so one run:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call pin '{}' \
  tools/call invoice_find "{\"where\": \"status = 'open' and due_on < now() - 30d\", \"aggregate\": {\"measures\": [{\"name\": \"overdue_gross\", \"func\": \"sum\", \"field\": \"gross\"}], \"group_by\": [\"partner\"]}}" \
  tools/call partner_get '{"key": "P-100", "with_meta": true}' \
  tools/call partner_get '{"key": "P-200", "with_meta": true}' \
  tools/call partner_get '{"key": "P-300", "with_meta": true}' \
  tools/call partner_get '{"key": "P-400", "with_meta": true}'
```

**3. `invoice_find`**:

```json
{
  "where": "status = 'open' and due_on < now() - 30d",
  "aggregate": {
    "measures": [{ "name": "overdue_gross", "func": "sum", "field": "gross" }],
    "group_by": ["partner"]
  }
}
```

Expected, at the pinned head:

```json
{ "shape": "groups", "rows": [
  { "by": { "partner": "P-100" }, "values": { "overdue_gross": "10710.00" } },
  { "by": { "partner": "P-200" }, "values": { "overdue_gross": "5950.00" } },
  { "by": { "partner": "P-300" }, "values": { "overdue_gross": "4760.00" } },
  { "by": { "partner": "P-400" }, "values": { "overdue_gross": "10710.00" } } ] }
```

The sums are exact decimals at `money`'s scale: `gross` is `net × 1.19`
kept to two digits, so every sum over it has two digits too.

**4. `partner_get`**, once per group — `{ "key": "P-100", "with_meta":
true }`, then P-200, P-300, P-400. Expected `credit_limit`: `10000.00`,
`5000.00`, `8000.00`, `20000.00`. With `with_meta`, each field says where
it came from, for example P-300's:

```json
"credit_limit": { "origin": "source", "source": "crm",
                  "seen_at": "2026-08-31T18:00:00.000000Z",
                  "position": 14, "writer": "…", "known_at": "…" }
```

`writer` is the principal the CLI ingested as (the root's owner), and
`known_at` the time of the ingest.

**The agent's answer:** P-100 Harbor Supplies (10710.00 overdue against
10000.00) and P-200 Northwind Tools (5950.00 against 5000.00), with
their credit limits taken from `crm`'s export of 31 August.

**5. `pin`** — `{ "as_of": 41 }` (the August ingest's position):
`{ "position": <head>, "pinned": 41 }`. Calls 5 to 8 are one turn, so one run:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call pin '{"as_of": 41}' \
  tools/call invoice_find "{\"where\": \"status = 'open' and due_on < now() - 30d\", \"aggregate\": {\"measures\": [{\"name\": \"overdue_gross\", \"func\": \"sum\", \"field\": \"gross\"}], \"group_by\": [\"partner\"]}}" \
  tools/call partner_get '{"key": "P-100", "with_meta": true}' \
  tools/call partner_get '{"key": "P-200", "with_meta": true}' \
  tools/call partner_get '{"key": "P-300", "with_meta": true}' \
  tools/call partner_get '{"key": "P-400", "with_meta": true}' \
  tools/call pin '{"clear": true}'
```

**6. `invoice_find`**, the same arguments. Expected, at position 41:
P-100 `10710.00`, P-200 `5950.00`, **P-300 `8330.00`**, P-400
`7140.00`.

**7. `partner_get`** for each group again: the same limits, since the
partners did not change.

**The agent's answer as of last month:** P-100, P-200 and **P-300 Cedar
Logistics** (8330.00 against 8000.00).

**8. `pin`** — `{ "clear": true }`: `{ "position": <head>, "pinned": null }`.

## Recording the walk

Write the result under this heading, in a pull request after the walk:
the date, the commit, every command's and call's actual output where it
differs from the expected one, and each rift as a finding with the issue
it was filed as. A walk with no rift says so.
