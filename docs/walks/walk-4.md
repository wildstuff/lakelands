# Walk 4 — a procedure runs on the lake

Walk 4 is the definition of done for `0.7.0`, as walk 3 was for
`0.4.0`. Walks 1 to 3 asked an agent's questions of a lake. Walk 4 runs
a small procedure on one, from an order to its payment. Starting from a
fresh clone, a person:

- builds the CLI and creates a shop lake with two customers and three
  orders;
- follows an order to its customer by a derived relation, with no link
  record;
- pays two orders, and sees a trigger guarded by the transition from
  `open` fire for each, and only for that transition;
- lets the host react: it calls `create_payment`, a verb on the type
  `payment` with no record, and writes the payment the ledger answered;
- runs the host again, and sees every reaction answer its first
  outcome, appending nothing;
- reads the lineage back from the payment to the order's change;
- retires a paid order, and sees nothing fire;
- reads a dashboard view for two business dates.

> **Which payment did paying order O-1 create, what led to it, and what
> did each customer pay in the week up to a given date?**

This page is the script, in the form of walk 3
(`docs/walks/walk-3.md`). Every command and every tool call is written
down with the answer it must give, derived by hand from the files below.
An answer that differs is a **rift**: the walk records it and goes on.

The walk needs no network. It uses the CLI, the `mcp_call` example
client, and one more example, `walk_4_host`, which plays the host. Two
things are the host's alone. A host binds the executor of a verb that
reaches outside the lake, and a host names the cause of a reaction
(`docs/decisions/021-the-write-context-belongs-to-the-host.md`); no door
carries a cause. `walk_4_host` does both through the store's own API,
with one in-process executor, as a host would.

## The materials

`examples/shop/` holds the bundle and the data:

| File | What it holds | What it shows |
|---|---|---|
| `types/customer.yaml` | `customer`, keyed by `number`: `number` and `name`, both `source` | |
| `types/order.yaml` | `order`, keyed by `id`. `id`, `customer_no`, `amount` and `ordered_on` are `source`; `status` is `shared`, written by the shop's export and by the agent; `paid_on` is `authored`. The relation `placed_by` to `customer` is **derived**: an order's `customer_no` matches a customer's `number` | a derived relation (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`) |
| `types/payment.yaml` | `payment`, keyed by the writer: `order_id`, `customer_no`, `amount`, `paid_on`, all `authored` | |
| `types/revenue_7d.yaml` | a view over `payment`, keyed by `customer_no`, with the parameter `anchor` (`date_time`). Its `where` is `paid_on > param('anchor') - 7d and paid_on <= param('anchor')`, grouped by `customer_no`, with measures `total = sum(amount)` and `payments = count()`. A view declares the fields it computes | a view's parameters, bound by each read (`docs/decisions/018-one-pattern-query.md`, extended) |
| `verbs/create_payment.yaml` | `create_payment` on the **type** `payment` (`scope: type`), risk `low`, payload `order_id`, executor `ledger` | a verb with no record (`docs/decisions/013-verbs-run-by-declared-risk.md`, extended) |
| `triggers/order_paid.yaml` | on `order`, `previous(status) = 'open' and status = 'paid'`, `max_depth: 1` | a transition guard, and a ceiling on the reactions to it |
| `triggers/order_settled.yaml` | on `order`, `status = 'paid' and not retired()` | a guard that tells a retirement from an update |
| `data/customers.ndjson` | C-1 Harbor Supplies, C-2 Delta Metals | |
| `data/orders.ndjson` | O-1 (C-1, 120.00, ordered 2026-09-21), O-2 (C-2, 80.00, 2026-09-24), O-3 (C-1, 45.50, 2026-09-30), all `open` | |

`crates/lakelands-cli/examples/walk_4_host.rs` is the host. It opens
the lake with the executor `ledger` bound, which answers
`{"payment": "PAY-<order>"}` for the order its payload names. It reads
the feed once from a given position and, for every firing of
`order_paid`, makes two writes:
- it calls `create_payment` with no key, naming the firing as the
  call's cause;
- it writes the payment the ledger answered, naming the call as its
  cause.

Each write carries an idempotency key, so the host may run again.

## The answer by hand

- **Paying O-1 and O-2** moves each from `open` to `paid`, so
  `order_paid` holds for each, and `order_settled` too.
- **Writing O-1's date again** leaves `status` at `paid`. `previous(status)`
  is `paid`, so `order_paid` does not hold; `order_settled` holds again.
- **The host's reaction** to each `order_paid` firing:
  - one `verb_called` with no key, caused by the firing;
  - the ledger's answer, recorded;
  - one payment, caused by the call: `PAY-O-1` for 120.00 from C-1, paid
    2026-09-29 (O-1's date as corrected), and `PAY-O-2` for 80.00 from
    C-2, paid 2026-10-03.
- **A second run** finds both firings again. Every write it would make
  carries a key already used, so each answers its first outcome:
  - the call, at its own position with the ledger's recorded answer;
  - the payment, at its own position.
- **The lineage** of `PAY-O-1` runs: the payment's write → the call →
  the firing → O-1's change to `paid`.
- **Retiring O-1** writes nothing new into its fields:
  - `order_paid` does not hold, since `previous(status)` is `paid`;
  - `order_settled` does not hold, by `not retired()`. Without that
    clause it would fire again, since `status = 'paid'` still holds on
    the retired record.
- **`revenue_7d`** reads `payment`, which no retirement touched.
  - At 2026-09-30 its window is (2026-09-23, 2026-09-30], which holds
    `PAY-O-1` alone: C-1, 120.00, one payment.
  - At 2026-10-07 the window is (2026-09-30, 2026-10-07], which holds
    `PAY-O-2` alone: C-2, 80.00, one payment.
  - A date alone is midnight UTC.

**The answer:**
- Paying O-1 created `PAY-O-1`, for 120.00, in Harbor Supplies' name.
- It was created by the ledger when the host called `create_payment`.
- The host did so because O-1 moved from `open` to `paid`.
- In the week up to 2026-09-30, Harbor Supplies paid 120.00. In the
  week up to 2026-10-07, Delta Metals paid 80.00.

## The commands

From a fresh clone, in the repository's root. As in walks 2 and 3,
`cargo install` replaces a `lakelands` already installed; to keep that
one, add `--root <dir>` and put `<dir>/bin` first on your `PATH`.

```sh
cargo install --path crates/lakelands-cli
lakelands init walk
cd walk
lakelands lake new shop
lakelands apply ../examples/shop
lakelands ingest ../examples/shop/data/customers.ndjson --source shop
lakelands ingest ../examples/shop/data/orders.ndjson --source shop
```

| Command | Expected |
|---|---|
| `init`, `lake new` | the root and the lake's location, exit 0 |
| `apply` | `applied 7 declaration(s), in force from position 10:`, then the seven: types `customer`, `order`, `payment`, `revenue_7d`, verb `create_payment`, triggers `order_paid`, `order_settled`. They are one append at 4 to 10, and every event of it carries `v` 8. The format is raised for the whole append that needs it (`docs/log-format.md` §6) |
| customers ingest | `committed 2 record(s), 0 already in the lake, through position 13`: the lease at 11, the customers at 12 and 13 |
| orders ingest | `committed 3 record(s), 0 already in the lake, through position 17`: the lease at 14, the orders at 15 to 17 |

The positions come from a run on develop at `450b911`.
`crates/lakelands-cli/tests/walk_4.rs` holds them: it runs these
commands with the built binary, one process each, as the walker does.
As in the earlier walks, every process that writes appends one
`lease_taken` first, and `feed` only reads.

## An order and its customer

The agent follows O-1 to its customer, in a session whose lease is 18:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call query '{"start": {"type": "order", "key": "O-1"}, "steps": [{"relation": "placed_by"}]}' \
  tools/call describe '{}'
```

**1. `query`** at 18: one row. Among its columns:
- `order.id` O-1 and `order.customer_no` C-1;
- `customer.number` C-1 and `customer.name` Harbor Supplies.

**2. `describe`** at 18: `order`'s relation `placed_by` carries
`"derived": {"match": [{"from": "customer_no", "to": "number"}]}`. No
type `sys.link.order.placed_by` exists: a derived relation has no link
records to keep.

## Paying two orders

The agent pays O-1 and O-2, then corrects O-1's date, in a session
whose lease is 19:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call order_upsert '{"key": "O-1", "fields": {"status": "paid", "paid_on": "2026-09-28"}}' \
  tools/call order_upsert '{"key": "O-2", "fields": {"status": "paid", "paid_on": "2026-10-03"}}' \
  tools/call order_upsert '{"key": "O-1", "fields": {"paid_on": "2026-09-29"}}'
lakelands feed
```

**3. – 5. `order_upsert`**: applied at 20, 23 and 26. The feed shows
what each write fired, in the same append:

| Position | Event | Cause |
|---|---|---|
| 20 | `record_upserted` O-1, version 2 | |
| 21 | `trigger_fired` `order_paid` O-1 | 20 |
| 22 | `trigger_fired` `order_settled` O-1 | 20 |
| 23 | `record_upserted` O-2, version 2 | |
| 24 | `trigger_fired` `order_paid` O-2 | 23 |
| 25 | `trigger_fired` `order_settled` O-2 | 23 |
| 26 | `record_upserted` O-1, version 3 | |
| 27 | `trigger_fired` `order_settled` O-1 | 26 |

The write at 26 changes `paid_on` only, so `previous(status)` is
already `paid`, and `order_paid` does not fire a second time.

## The host reacts

```sh
cargo run -q -p lakelands-cli --example walk_4_host -- --lake shop --from 20
lakelands feed
```

`walk_4_host` prints one line per reaction:

```
order `O-1`: fired at 21, create_payment at 29 answered {"payment":"PAY-O-1"}, payment `PAY-O-1` at 31
order `O-2`: fired at 24, create_payment at 32 answered {"payment":"PAY-O-2"}, payment `PAY-O-2` at 34
```

Its lease is at 28. For O-1:

| Position | Event | Cause | Context, writer |
|---|---|---|---|
| 29 | `verb_called` `create_payment`, payload `{"order_id": "O-1"}`, **no `key`** | 21 | reactive, depth 1; `service:walk_4_host` |
| 30 | `output_recorded` `{"payment": "PAY-O-1"}`, seam `verb:ledger:create_payment` | 29 | the store's own |
| 31 | `record_upserted` payment `PAY-O-1`: `order_id` O-1, `customer_no` C-1, `amount` 120.00, `paid_on` 2026-09-29 | 29 | reactive, depth 2; `service:walk_4_host` |

O-2's reaction is the same at 32 to 34, caused by 24.

The call's cause is the firing because the host names it. The store
checks only that the position is at or below the head. `order_paid`'s
`max_depth: 1` admits the call at depth 1. The payment is written at
depth 2, and its cause is the call, not a firing, so the ceiling does
not apply to it.

## The host again

```sh
cargo run -q -p lakelands-cli --example walk_4_host -- --lake shop --from 20
```

It prints the same two lines. Each call is answered by its idempotency
key (`create_payment:O-1`, `create_payment:O-2`) with its first
outcome: the `verb_called`'s own position, 29 and 32, and the ledger's
answer recorded at 30 and 33. The executor does not run again. Each
payment is answered at 31 and 34. The run appends its lease at 35 and
nothing else.

## The lineage, read back

The agent reads the payment with its metadata, in a session whose
lease is 36:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call payment_get '{"key": "PAY-O-1", "with_meta": true}'
```

**6. `payment_get`** at 36: the fields above. Every field's metadata
names `"position": 31` and `"writer": "service:walk_4_host"`.

From 31, the feed's `cause` members lead back, one event at a time:
- 31, the payment, has cause 29;
- 29, the call, has cause 21;
- 21, `order_paid` firing, has cause 20;
- 20 is O-1's change to `paid`.

## Retiring a paid order

In a session whose lease is 37:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call order_retire '{"key": "O-1"}'
lakelands feed
```

**7. `order_retire`**: applied at 38, version 4. The feed ends with the
`record_retired` at 38. No `trigger_fired` follows: `order_paid` does
not hold, and `order_settled`'s `not retired()` holds back the firing a
plain `status = 'paid'` would make here.

## A dashboard for two dates

The agent reads `revenue_7d` for two business dates, in a session whose
lease is 39:

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call revenue_7d_find '{"params": {"anchor": "2026-09-30"}}' \
  tools/call revenue_7d_find '{"params": {"anchor": "2026-10-07"}}'
```

**8. `revenue_7d_find`** at 2026-09-30, at 39: one record, key `C-1`,
`{"customer_no": "C-1", "payments": 1, "total": "120.00"}`.

**9. `revenue_7d_find`** at 2026-10-07, at 39: one record, key `C-2`,
`{"customer_no": "C-2", "payments": 1, "total": "80.00"}`.

The view is computed on read, so it appends nothing. The two windows
are two answers at one position: what the view holds is the reader's
date, not the lake's.

The same read without its parameter, in a session of its own (lease
40):

```sh
cargo run -q -p lakelands-server --example mcp_call -- --server "lakelands serve --mcp" \
  tools/call revenue_7d_find '{}'
```

**10.** Refused, and `mcp_call` stops with

```
error: refused: view `revenue_7d` needs parameter `anchor` (date_time)
  what to do: correct the input and run the command again; nothing was written
```

**The agent's answer:**

- Paying O-1 created `PAY-O-1`, 120.00 from Harbor Supplies (C-1).
  The ledger created it when the host called `create_payment` at 29. The
  host did so for the firing of `order_paid` at 21, which O-1's change to
  `paid` at 20 caused.
- In the week up to 2026-09-30 Harbor Supplies paid 120.00, in one
  payment. In the week up to 2026-10-07 Delta Metals paid 80.00, in one.

## Recording the walk

Write the result under this heading, in a pull request after the walk:

- the date and the commit;
- every command's and call's actual output where it differs from the
  expected one;
- each rift as a finding, with the issue it was filed as.

A walk with no rift says so.
