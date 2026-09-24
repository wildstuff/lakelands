# Walk 2 — what the collector sees on 1 December

Walk 2 is the definition of done for `0.2.0`, as walk 1 was for `0.1.0`.
Starting from a fresh clone, a person:

- builds the CLI and a loopback fake;
- creates a lake whose root names an `exec` program, an embedding
  endpoint and a webhook endpoint;
- applies an ontology and ingests one export;
- ages the lake to a fixed date;
- sees the deliverer post what fired;
- has an agent link invoices to their approvers, search by meaning and
  read the lake as it stood before the aging.

> **Which invoices went overdue on 1 December, who approved them and
> since when, and which invoices are about freight, even where the
> summary never uses the word?**

This page is the script, in the form of walk 1
(`docs/walks/walk-1.md`). Every command and every tool call is written
down with the answer it must give. Every answer is derived by hand from
the files below and from the fake's rules. An answer that differs is a
**rift**: the walk records it and goes on.

**The clock plays no part.** Every date the lake reasons about is handed
to it: by the export and by `lakelands age --today 2026-12-01`. The
answers below hold on any day the walk is run. Only `known_at`, the
time a fact was written, differs from run to run.

## The materials

`examples/collections/` is the ontology bundle, with its data under
`data/` and the root's configuration beside it:

| File | What it holds |
|---|---|
| `value_types/money.yaml` | `money`: a decimal kept at two fractional digits, as in walk 1 |
| `value_types/invoice_status.yaml` | `invoice_status`: text, one of `open`, `overdue`, `paid` |
| `types/partner.yaml` | `partner`, keyed by `id`: `name`, `credit_limit` |
| `types/invoice.yaml` | `invoice`, keyed by `number`. Its fields: `partner`, `issued_on`, `due_on`, `net`; `gross` (derived: `net * 1.19`); `band` (computed by the function `band`); `status` (shared); `summary` (searchable); `summary_vec` (a `vector(4)`, the embedding of `summary` under the model `walk-fake`). Its relations: `billed_to` via `partner`, and `approved_by` to `partner`, carried by link records with a `since` (`docs/decisions/035-a-link-is-a-record.md`) |
| `functions/band.yaml` | `band`: backing `exec`, `config: { command: band }`, input `gross` |
| `triggers/went_overdue.yaml` | fires when `status` changes to `overdue` |
| `aging_rules/open_to_overdue.yaml` | on `invoice`, moves `status` from `open` to `overdue` when `due_on < today() - 30d`; `apply` writes it as a `sys.aging_rule` record |
| `data/partners.csv`, `data/invoices.csv` | three partners and six invoices, exported on 30 November 2026 |
| `data/partners.map.json`, `data/invoices.map.json` | the column maps `lakelands ingest --map` takes |
| `lakelands.json` | the root's configuration: the `exec` command `band`, the `embed` endpoint and the `deliver` endpoint `collector`. `<fake>` stands for the fake's absolute path |

**The fake.** `crates/lakelands-cli/examples/walk_2_fake.rs` is one
binary with three modes, written on the standard library and
`serde_json`, which the CLI already uses. Each of its answers can be
worked out by hand:

- `walk_2_fake embed 127.0.0.1:8701` is a `/v1/embeddings` endpoint.
  The vector of a text counts the words of four groups in it, one
  dimension each:
  1. `freight`, `shipping`, `haulage`
  2. `software`, `licence`
  3. `food`, `catering`
  4. `repair`, `service`
- `walk_2_fake hook 127.0.0.1:8702 hooks.jsonl` receives the deliveries
  and appends one line per delivery: the lake and position from the
  deliverer's headers, and the event's body.
- `walk_2_fake band` is the `exec` program. It reads the call from
  standard input and answers `"low"` for a `gross` under 2000, `"mid"`
  under 5000 and `"high"` from 5000.

**Why the fakes run first.** A function or embedding call that fails is
recorded as a failure and is final for its input. If the embedding
endpoint is down during the ingest, `summary_vec` stays empty with the
error in its metadata. Only a later `apply` of the bundle calls again.
Start both fake servers before the first `ingest`.

## The data, and the answer by hand

Partners (`partners.csv`, source `crm`): P-100 Harbor Supplies, P-200
Northwind Tools, P-300 Cedar Logistics.

Invoices (`invoices.csv`, source `erp`):

- `gross` is `net × 1.19`, and `band` is read off `gross`.
- The vector comes from the words of `summary`.
- On 1 December, "overdue" means open with `due_on` before 1 November
  (`today() - 30d`). `today()` is 1 December at midnight, and the
  comparison is strict.

| Invoice | Partner | Due | Net | Gross | Band | Status | Summary | Vector | Aged on 1 Dec |
|---|---|---|---|---|---|---|---|---|---|
| INV-5001 | P-300 | 2026-09-30 | 4000.00 | 4760.00 | mid | open | Freight from Rotterdam | [1,0,0,0] | **overdue** |
| INV-5002 | P-300 | 2026-10-15 | 1200.00 | 1428.00 | low | open | Haulage and shipping in October | [2,0,0,0] | **overdue** |
| INV-5003 | P-100 | 2026-10-31 | 6000.00 | 7140.00 | high | open | Catering for the harbour opening | [0,0,1,0] | **overdue** |
| INV-5004 | P-100 | 2026-11-01 | 800.00 | 952.00 | low | open | Software licence renewal | [0,2,0,0] | no: due on the boundary, not before it |
| INV-5005 | P-200 | 2026-10-01 | 2500.00 | 2975.00 | mid | paid | Repair of the loading crane | [0,0,0,1] | no: not `open` |
| INV-5006 | P-200 | 2026-12-15 | 3000.00 | 3570.00 | mid | open | Freight insurance and service | [1,0,0,1] | no: not yet due |

**The search for `shipping`**, hybrid, `k: 3`:

- The fake embeds the query as `[1,0,0,0]`.
- By cosine, INV-5001 and INV-5002 score 1, INV-5006 scores 0.707 and
  the rest score 0. The tie between INV-5001 and INV-5002 is broken by
  key.
- The lexical ranking holds only INV-5002, the one summary with the word
  `shipping`.
- Reciprocal rank fusion adds `1 / (60 + rank)` per list:
  - INV-5002: 1/60 + 1/61 = **0.03306**
  - INV-5001: 1/60 = **0.01667**
  - INV-5006: 1/62 = **0.01613**

INV-5001 is found although its summary never says `shipping`.

**The answer:** INV-5001 and INV-5002 of Cedar Logistics and INV-5003 of
Harbor Supplies went overdue on 1 December. INV-5001 was approved at
P-300 since 1 September, and INV-5003 at P-100 since 15 October.
INV-5002 has no approver. The invoices about freight are INV-5002,
INV-5001 and INV-5006, in that order.

## The commands

From a fresh clone, in the repository's root. `cargo install` puts the
fake where the configuration can name it by an absolute path, since
`exec` runs no program given by a relative one. The `walk` directory
may be created anywhere; the page creates it inside the clone.

```sh
cargo install --path crates/lakelands-cli
cargo install --path crates/lakelands-cli --example walk_2_fake
walk_2_fake embed 127.0.0.1:8701 &
lakelands init walk
cd walk
walk_2_fake hook 127.0.0.1:8702 hooks.jsonl > /dev/null &
lakelands lake new collections
sed "s|<fake>|$(command -v walk_2_fake)|" ../examples/collections/lakelands.json > lakelands.json
lakelands apply ../examples/collections
lakelands ingest ../examples/collections/data/partners.csv \
  --source crm --map ../examples/collections/data/partners.map.json
lakelands ingest ../examples/collections/data/invoices.csv \
  --source erp --map ../examples/collections/data/invoices.map.json
lakelands feed --kind output_recorded
```

What each must print:

| Command | Expected |
|---|---|
| `init`, `lake new` | the root and the lake's location, exit 0 |
| `apply` | `applied 6 declaration(s), in force from position 10`, then the six: value types `invoice_status`, `money`, types `invoice`, `partner`, function `band`, trigger `went_overdue`; then ``aging rule `open_to_overdue` written at position 11`` |
| partners ingest | one `committed` line: **3 records, 0 already in the lake**, through position **15** |
| invoices ingest | one `committed` line: **6 records, 0 already in the lake**, through position **34**. That covers the six invoices (17 to 22), then the twelve calls the ingest made after its append: six `band` outputs and six embeddings |
| `feed --kind output_recorded` | twelve events, 23 to 34: `band` for INV-5001 (`"mid"`) at 23; the six embeddings, INV-5001 to INV-5006 in order, at 24 to 29 (the vectors in the table above); `band` for INV-5002 to INV-5006 at 30 to 34 (`low`, `high`, `low`, `mid`, `mid`). Each `band` event has `seam` `function:exec:band` and `procedure` `band`. Each embedding has `seam` `embed:walk-fake` |

The positions come from a run on develop at `959d4ec`, after lakelands#67, which
declares `sys.aging_rule` in every new lake beside `sys.setting`; `apply`
now also writes the bundle's aging rule, one position after its
declarations, so every position after `apply` is one later than a
bundle without the rule would give. `crates/lakelands-cli/tests/walk_2.rs`
holds them: it runs these
commands with the built binary, one process each, as the walker does.
Every command that writes appends one `lease_taken` first, as in walk 1.

## The agent links two invoices

As in walk 1, the agent's calls go through the `mcp_call` example
client from the `walk` directory. The client starts `lakelands serve
--mcp` itself and prints each reply's position line and its whole
envelope, `{ "data": …, "position": N }`. Every `serve --mcp` appends
one `lease_taken`. It also runs the deliverer of `lakelands.json` for as
long as it serves.

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call invoice_approved_by_link '{"key": "INV-5001", "to": "P-300", "fields": {"since": "2026-09-01T00:00:00Z"}}' \
  tools/call invoice_approved_by_link '{"key": "INV-5003", "to": "P-100", "fields": {"since": "2026-10-15T00:00:00Z"}}'
```

**1. `invoice_approved_by_link`**, twice: `{ "outcome": "applied",
"version": 1 }` at positions **36** and **37**. Position 35 is this
session's `lease_taken`.

## Aging to 1 December

The walk ages before it serves. A lake has one writer, and a process
that holds it keeps it until it ends
(`docs/decisions/033-a-process-holds-a-lake-once.md`), so an `age`
beside a running `serve` would be refused. The page orders the steps so
that this never arises.

```sh
lakelands age --today 2026-12-01
lakelands age --today 2026-12-01
```

| Command | Expected |
|---|---|
| first `age` | `open_to_overdue: moved 3, 0 guard(s) failed`, then `aged 2026-12-01 through position 44`. After its `lease_taken` at 38, the rule's one append holds, for INV-5001, INV-5002 and INV-5003 in turn, a `record_upserted` (39, 41, 43) with the `procedure` `aging:open_to_overdue`, the writer `system:lakelands` and the context origin `scheduled`, each followed by its `went_overdue` firing (40, 42, 44) |
| second `age` | `open_to_overdue: moved 0, 0 guard(s) failed`, then `aged 2026-12-01 through position 45`: the head after the first `age` plus this process's own `lease_taken` and nothing else. Aging a date twice writes it once |

INV-5004 is due on 1 November, which is not before `today() - 30d`, so
it stays `open`. INV-5005 is `paid`, not `open`, so the rule does not
touch it. INV-5006 is not due yet.

## The deliverer posts what fired

`lakelands.json` names one endpoint, `collector`, for the lake's
`trigger_fired` events, starting from the lake's first event
(`start: genesis`). `serve` delivers for as long as it runs, so here it
must stay up with no client. Without a `server` section, `serve --mcp`
ends when its standard input does. A plain `serve` that runs only the
deliverer is refused ("nothing to serve was named"), and that is what
this step stands in for. `sleep 2 |` keeps standard input open for two
seconds, a margin chosen to cover a poll of 200 milliseconds and three
posts to loopback. lakelands#88 is the other face of the same gap: with
a `server` section, `serve --mcp` does not end with its client. The
step changes when that is settled:

```sh
sleep 2 | lakelands serve --mcp
cat hooks.jsonl
```

`hooks.jsonl` holds exactly **three** lines, in log order, one per
`went_overdue` firing, at positions **40**, **42** and **44**; this
`serve`'s own `lease_taken` is 46. Each line carries `"lake":
"collections"`, the firing's position, and `{"kind": "trigger_fired", "trigger":
"went_overdue", "type": "invoice", "key": …}` for INV-5001, INV-5002
and INV-5003. Nothing of the invoice's fields is posted: a firing
carries its key only. The deliverer keeps its place in
`deliveries/collections/collector.json`, so a second `serve` posts
nothing again.

## The agent's calls

Calls 2 to 6 are one session, whose `lease_taken` is 47, the head
every answer below reads at:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call invoice_find "{\"where\": \"status = 'overdue'\"}" \
  tools/call sys_link_invoice_approved_by_find '{}' \
  tools/call invoice_get '{"key": "INV-5001", "with_meta": true}' \
  tools/call search '{"type": "invoice", "mode": "hybrid", "text": "shipping", "field": "summary_vec", "k": 3}' \
  tools/call pin '{"as_of": 37}' \
  tools/call invoice_find "{\"where\": \"status = 'overdue'\"}" \
  tools/call pin '{"clear": true}'
```

**2. `invoice_find`** `{ "where": "status = 'overdue'" }`, at the head:
INV-5001, INV-5002 and INV-5003.

**3. `sys_link_invoice_approved_by_find`** `{}`: the two link records,
keyed by their pair:

```json
{ "shape": "records", "rows": [
  { "type": "sys.link.invoice.approved_by", "key": "INV-5001:P-300", "version": 1,
    "fields": { "from": "INV-5001", "to": "P-300", "since": "2026-09-01T00:00:00.000000Z" } },
  { "type": "sys.link.invoice.approved_by", "key": "INV-5003:P-100", "version": 1,
    "fields": { "from": "INV-5003", "to": "P-100", "since": "2026-10-15T00:00:00.000000Z" } } ] }
```

**4. `invoice_get`** `{ "key": "INV-5001", "with_meta": true }`:

- The fields: `gross` `"4760.00"`, `band` `"mid"`, `summary_vec`
  `[1, 0, 0, 0]`, and `status` `"overdue"`.
- The metadata of `band` says `"origin": "derived"`, `"position": 23`
  (the call's output) and `"writer": "system:lakelands"`. So does that of
  `summary_vec`, with `"position": 24`. That of `status` names the aging:
  `"origin": "system"`, `"position": 39` and `"writer":
  "system:lakelands"`.

**5. `search`**, hybrid, for `shipping` over `summary_vec`, `k: 3`:
`"ranked_by": "hybrid"`, hits INV-5002 (`0.03306…`), INV-5001
(`0.01666…`), INV-5006 (`0.01612…`), and a `next_cursor` for the three
not shown.

**6. `pin`** `{ "as_of": 37 }`, the second link's position, is the lake
as it stood before the aging. `invoice_find` for `overdue` there answers
no rows: `{ "shape": "records", "rows": [] }`. Then `pin` `{ "clear":
true }` releases the pin.

**The agent's answer:**

- INV-5001 (Cedar Logistics, 4760.00, approved at P-300 since
  1 September), INV-5002 (Cedar Logistics, 1428.00, no approver) and
  INV-5003 (Harbor Supplies, 7140.00, approved at P-100 since
  15 October) went overdue on 1 December. Before the aging, none was.
- About freight: INV-5002 and INV-5001, and INV-5006 after them.

## Found before the walk

- **With an HTTP door configured, `serve --mcp` outlives its client.**
  When the root's configuration has a `server` section, `serve --mcp`
  serves stdio and HTTP together and runs until both end. When
  `mcp_call` closes standard input, the process stays behind. It keeps
  the lake's writer lease, and the next command is refused as
  unavailable. Plain `lakelands serve` without `--mcp` is refused
  ("nothing to serve was named"). Filed as lakelands#88. This walk
  configures no `server`, so every `serve` ends with its client.
- **A failed call is final for its input.** If the fake is not
  listening, the ingest records each embedding as a failure, and the
  vectors stay empty until the bundle is applied again. The walk starts
  the fakes first.

## Recording the walk

Write the result under this heading, in a pull request after the walk:

- the date and the commit;
- every command's and call's actual output where it differs from the
  expected one;
- each rift as a finding, with the issue it was filed as.

A walk with no rift says so.
