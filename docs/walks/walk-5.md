# Walk 5: an LLM agent works a lake through MCP alone

The people who scripted walks 1 to 4 knew the materials and the code.
A host's agents know neither. An LLM agent sees only the tools a lake's
MCP door lists, their descriptions and schemas, and the answers to its
calls. Walk 5 asks whether that is enough to work a lake correctly:

> **Can an agent that sees nothing but the MCP door do a clerk's tasks
> on a lake, and when the lake refuses it, can it tell from the answer
> alone what to change?**

The walker is not a person. It is a **blind agent**: a fresh LLM session
whose only tools are the lake's MCP tools. It ran three times; each run
is one MCP session as the principal `clerk`, and the runs follow each
other on the same lake. Each step below is taken from the agent's
transcript as it happened. `crates/lakelands-cli/tests/walk_5.rs`
replays exactly those calls and pins every answer quoted here.

A step that tells the agent less than it needs is a **finding**, filed
as its own issue with the text as it is, what the agent did, and a
proposed text. The criterion for a refusal is whether the agent changed
the right thing on its first try.

## The agent

The agent is Claude Code in print mode (`claude -p`, claude-opus-5-5),
started in an empty directory outside the repository, so it has no
`CLAUDE.md`, no memory and no repository context:

```sh
claude -p "$(cat run-1.prompt.txt)" \
  --tools "" \
  --strict-mcp-config --mcp-config mcp.json \
  --allowedTools mcp__lake \
  --no-session-persistence --output-format stream-json --verbose < /dev/null
```

- **`--tools ""`** gives it no built-in tool, so it cannot run a shell or
  read a file.
- **`--strict-mcp-config`** gives it exactly one MCP server, the lake:

```json
{"mcpServers": {"lake": {"command": "lakelands",
  "args": ["--root", "<walk>", "--as", "clerk", "serve", "--mcp"]}}}
```

Its blindness is therefore a matter of what it can do, not of what its
prompt asks.

`docs/walks/walk-5/` holds the record:

| File | What it holds |
|---|---|
| `run-N.prompt.txt` | the prompt of run N, word for word |
| `run-N.jsonl` | every message of run N as `stream-json` wrote it: the agent's thinking, its text and its calls, and every answer, in order. Calls, arguments and answers are word for word. Left out, because they say nothing about the walk and this repository is public: the harness's own lines (session set-up, rate limits, usage); the session, message and request ids; the encrypted signature of each thinking block, and the thinking blocks with no text; token counts; and the harness's copies of each call and answer |
| `run-N.txt` | the same, rendered as text by `render.py` |
| `tools.json` | the tool list the clerk sees, with every description and schema |

**The three runs.**
- **Run 1** gives the agent a clerk's tasks in plain words. It read
  `describe` first, knew from it which fields it may write, and so met
  almost no refusal.
- **Run 2** starts each task with a colleague's faulty first call, given
  word for word, so the agent meets each common refusal. What it does
  after the refusal is its own.
- **Run 3** does the same for a value of the wrong kind, which the agent
  had avoided in run 2 by reading the schema.

## The materials

| File | What it holds |
|---|---|
| `examples/shop/` | walk 4's bundle and data, unchanged (`docs/walks/walk-4.md`) |
| `examples/walk-5/verbs/cancel_order.yaml` | `cancel_order` on `order`, risk **medium**, `requires: status = 'open'`, sets `status` to `cancelled`. The shop has no medium-risk verb |
| `examples/walk-5/principal.json` | the principal `clerk` (kind `agent`). It may read everything, write `order.status` and only that field of `order`, and write `payment`. It holds no review, schema or ingest grant |

A bundle must declare every type it names, so `cancel_order` cannot be
applied on its own. The walk copies the shop, adds the verb, and applies
the copy. `examples/shop/` stays as walk 4 has it, and walk 4's
positions do not move.

## The commands

From a fresh clone, in the repository's root:

```sh
cargo install --path crates/lakelands-cli
lakelands init walk
cd walk
lakelands lake new shop
cp -r ../examples/shop shop
cp ../examples/walk-5/verbs/cancel_order.yaml shop/verbs/
lakelands apply shop
lakelands ingest shop/data/customers.ndjson --source shop
lakelands ingest shop/data/orders.ndjson --source shop
```

Then add `examples/walk-5/principal.json` to the `principals` list of
`walk/lakelands.json`.

| Command | Answer |
|---|---|
| `apply` | `applied 8 declaration(s), in force from position 11:` types `customer`, `order`, `payment`, `revenue_7d`, verbs `cancel_order`, `create_payment`, triggers `order_paid`, `order_settled` |
| customers ingest | `committed 2 record(s), 0 already in the lake, through position 14` |
| orders ingest | `committed 3 record(s), 0 already in the lake, through position 18` |

As the clerk, the lake lists 24 tools (`tools.json`). The clerk gets no
`customer_upsert` and no `accept`, and `order_upsert`'s `fields` lists
only `status`.

## Run 1: a clerk's tasks

The prompt (`run-1.prompt.txt`) sets "today" to 2026-10-07 and asks for:

1. what the store holds and what the agent may do;
2. the customer Delta Metals and its orders;
3. O-2 marked shipped;
4. O-2 recorded as paid yesterday;
5. O-3 cancelled;
6. the ledger payment for O-1;
7. each customer's revenue for the seven days up to today.

The session's lease is at 19.

| # | Call | Answer | What the agent made of it |
|---|---|---|---|
| 1 | `describe {}` | the lake at 19. `may` says no ingest, review or schema; `order.status` has access `write`, `order.paid_on` has `read`; `cancel_order` is risk `medium`, `create_payment` has scope `type` | "I'm limited to updating order status, calling cancel_order/create_payment, and writing payments" |
| 2–3 | `sys_aging_rule_find {}`, `sys_setting_find {}` | no rows, at 19 | |
| 4 | `customer_find {"where": "name = 'Delta Metals'"}` | C-2 | |
| 5 | `order_find {"with_meta": true}` | O-1, O-2 (C-2, 80.00, `open`), O-3 | |
| 6 | `order_upsert` O-2 `status: shipped`, `expected_version: 1`, with an idempotency key | `applied`, position 20, version 2 | |
| 7 | `order_upsert` O-2 `status: paid`, `expected_version: 2` | `applied`, position 21, version 3 | "Since paid_on is read-only, I'll update its status to paid instead" |
| 8 | `payment_find {"with_meta": true}` | no rows, at **22** | "the head moved from 21 to 22, so something else happened" (see below) |
| 9 | `order_get` O-2 with meta | `status: paid`, written by `agent:clerk` at 21; read at 22 | |
| 10 | `create_payment {"payload": {"order_id": "O-2"}}` | refused: executor not bound (below) | it does not write a payment by hand: "writing a fake `payment` row would misrepresent an actual ledger transaction" |
| 11 | `cancel_order` O-3, `expected_version: 1` | `proposed` at 23, with the reason quoted below | "pending reviewer approval since I don't have review authority" |
| 12 | `create_payment` for O-1 | refused: executor not bound | |
| 13 | `revenue_7d_find {"params": {"anchor": "2026-10-07T00:00:00Z"}}` | no rows, at 23 | "every customer's revenue is 0.00" |

The answer to steps 10 and 12:

```
error: nothing is bound for the verb executor `ledger`
  what to do: bind it in the host's configuration

{"error":"unbound","seam":"the verb executor `ledger`"}
```

`lakelands serve` binds no verb executor, so `create_payment` cannot
run. Yet its description reads "Create the payment of a paid order in
the ledger. It runs at once." The answer's next step addresses an
operator, not the caller. The agent stopped rightly, but nothing told it
that before it called. **Finding:** #349.

Since #349:
- the tool says "Create the payment of a paid order in the ledger. It
  runs at once, through an executor the lake's host binds; where none is
  bound, a call is refused and writes nothing.";
- the answer's next step reads "nothing was written; this cannot run
  until the lake's operator binds it in the host's configuration".

`walk_5.rs` pins the new step.

The answer to step 11:

```json
{"outcome":"proposed","position":23,"proposal":"79ec049b2abb8192a91d58b7b12cc85d",
 "why":"`cancel_order` is declared medium risk; it runs when accepted"}
```

The agent understood it at once: O-3 stays `open` until someone with
review rights accepts the proposal. The answer was clear enough.

The agent's own report: tasks 1, 2, 3 and 7 done; task 4 partly (the
status, not the date); task 5 waiting for review; task 6 not done.

## Run 2: a colleague's faulty first calls

The prompt (`run-2.prompt.txt`) gives six tasks, each with the call a
colleague made first. The agent makes that call word for word, then
finishes the task on its own. The session's lease is at 24.

### 1. An unknown field

`order_upsert {"key": "O-1", "fields": {"state": "shipped"}}`:

```
error: refused on `state`: `order` has no field `state`
  what to do: correct the input and run the command again; nothing was written

{"error":"refused","field":"state","reason":{"message":"`order` has no field `state`","refusal":"invalid"}}
```

Next it sent `status: shipped`, which was applied at 25, version 2.
**First try right.** The agent took the name from the tool's schema
(`fields` lists only `status`), and called the answer itself "partly"
clear, since it names the wrong field and not the right one. By the
walk's criterion this is not a finding.

### 2. A payment written by hand, with no key

`payment_upsert` with `fields` only (and `amount: 80`):

```
error: the arguments do not fit `payment_upsert`: missing field `key`
  what to do: correct them against the tool's input schema and call it again
```

The agent listed the payments (none, at 25) and described `payment`,
whose `access` names no key. It chose `O-2` itself and wrote with
`expected_version: 0`, `amount: "80.00"` and a date-time `paid_on`: the
payment was applied at 26, version 1. It warned that "if the ledger's
own convention turns out to be different, that record may need
re-keying". **Not first try right.** Nothing told it what the key of a
type without one is. **Finding:** #350.

Since #350, `payment_upsert`'s `key` says: "`payment` declares no key,
so the key is yours to choose. It names this record from now on, and a
later write with the same key changes it." A keyed type's names the
field whose value it is (`order`: "the value of its `id`").

### 3. A field the clerk may not write

`order_upsert {"key": "O-2", "fields": {"paid_on": "2026-10-06"}}`:

```
error: refused on `paid_on`: you hold no grant for this
  what to do: act as a principal whose grants allow it, or ask the lake's owner for a grant

{"error":"refused","field":"paid_on","reason":{"refusal":"not_granted"}}
```

The agent tried to route the write through review with `propose`. Its
first proposal had the shape `order_upsert` takes (`fields`):

```
error: the arguments do not fit `propose` at `write`: missing field `patch`
  what to do: correct them against the tool's input schema and call it again
```

With `patch`, the proposal was refused the same way as the write, since
a proposal does not lift a grant. The agent then reported the task as
not doable from this session. The refusal itself was **clear**. But
`propose` showed neither the shape of a record write nor that it never
lifts a grant, which cost two calls. **Finding:** #351.

### 4. A stale version

`order_upsert {"key": "O-1", "fields": {"status": "on hold"}, "expected_version": 1}`:

```
error: the record changed: expected version 1, found 2
  what to do: read the record again and retry against its current version

{"actual":2,"error":"conflict","expected":1}
```

The agent read O-1, saw that version 2 was its own write at 25 ("no
other changes to O-1 since then"), and retried at version 2: applied at
27, version 3. **First try right.**

### 5. A key on a verb called on its type

`create_payment {"key": "O-1", "payload": {"order_id": "O-1"}}`:

```
error: refused: verb `create_payment` is called on the type `payment`, not on a record: it takes no key
  what to do: correct the input and run the command again; nothing was written

{"error":"refused","reason":{"message":"verb `create_payment` is called on the type `payment`, not on a record: it takes no key","refusal":"invalid"}}
```

It dropped the key and added an idempotency key (**first try right**),
and then met the executor that is not bound, as in run 1 (#349).

### 6. A view read without its parameter

`revenue_7d_find {}`:

```
error: refused: view `revenue_7d` needs parameter `anchor` (date_time)
  what to do: correct the input and run the command again; nothing was written

{"error":"refused","reason":{"message":"view `revenue_7d` needs parameter `anchor` (date_time)","refusal":"invalid"}}
```

It retried with `{"params": {"anchor": "2026-10-07"}}`. The answer, at
27, was C-2 with `total` `80.00` from one payment, the one it wrote in
task 2. **First try right.** A date without a time is taken as
midnight UTC.

## Run 3: a value of the wrong kind

The prompt (`run-3.prompt.txt`) gives one task with a colleague's call:
`payment_upsert` under the key `P-O-1` with `amount: 120`. The session's
lease is at 28.

```
error: refused on `amount`: a decimal value must be a string of digits
  what to do: correct the input and run the command again; nothing was written

{"error":"refused","field":"amount","reason":{"message":"a decimal value must be a string of digits","refusal":"constraint"}}
```

The agent sent `"120.00"` and changed nothing else: applied at 29,
version 1. Read back, the payment's `paid_on` is
`2026-10-07T00:00:00.000000Z`. **First try right**, and in the agent's
words: "the refusal alone told me what to change".

## Found on the walk

| Step | Finding | Issue |
|---|---|---|
| run 1, steps 10 and 12; run 2, task 5 | A verb whose executor is not bound is described as running at once, and its refusal tells the caller to change the host's configuration | #349 |
| run 2, task 2 | An upsert's `key` has no description, and nothing tells the caller what to choose for a type without a key | #350 |
| run 2, task 3 | `propose` does not show a record write's shape, and does not say that a proposal never lifts a grant | #351 |
| the walk's set-up | `mcp_call`, the client walks 2 to 4 read the door with, shows only a refusal's text and stops its session at it. A reader of those pages sees less than an agent | #348 |

**Observations, with no issue:**

- **The unknown field (run 2, task 1).** The answer names the field
  that does not exist, not the fields the caller may write. The agent
  found the right one in the schema at once.
- **A read answered at a later position (run 1, step 8).** The write at
  21 set O-2 to `paid`, and in the same append the trigger
  `order_settled` fired at 22 (cause 21). The write's answer says
  `position: 21`. The next read answers at 22, and the agent wondered
  what had happened there. Nothing on the door tells it.
  - A later feature could let a write's answer say where its append
    ended and what it fired, for example `"through": 22,
    "fired": ["order_settled"]`.
  - That would extend the answer's contract, so it waits until the
    door is stable.
- **`describe` first.** Run 1 shows the door at its best: from one
  `describe`, the agent knew what it could write, what needs review and
  what it may not touch, and it kept to that without a refusal.

## Recording the walk

The runs were made on 2026-10-02 against develop at 0e4209c.
`crates/lakelands-cli/tests/walk_5.rs` sets the lake up as above and
replays each run as one MCP session, with exactly the agent's calls.
It pins every answer this page quotes: each refusal's text and JSON,
and each position. A refusal reworded by a fix of #349, #350 or #351
turns it red, and the fix's pull request updates the step here and in
the test.
