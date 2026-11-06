# Walk 6: an agent's graph tasks through the typed query tool and through Cypher

`lakelands-cypher` reads a statement of an openCypher subset and
translates it into the one pattern query (`docs/decisions/018-one-pattern-query.md`).
REST and the CLI take such a statement; no MCP tool does, and whether
one should is left to a walk
(`docs/decisions/082-a-query-language-writes-only-through-the-typed-write-doors.md`).
This is that walk:

> **Does an agent answer graph questions better through a statement of
> the subset than through the typed `query` tool, measured on the same
> tasks, the same lake and the same agent?**

The walker is walk 5's blind agent (`docs/walks/walk-5.md`): a fresh
LLM session whose only tools are a lake's MCP tools. It ran four
times, twice on each door, each run on a fresh lake. The answers below
are taken from its transcripts as they happened. The walk is a
measurement, not a contract: no test replays it.

A refusal or a tool description that told the agent less than it
needed, or something wrong, is a **finding**, filed as its own issue
with the text as it is, what the agent did and a proposed text. A
refusal is **understood** when the agent's next call changed the right
thing.

## The agent

The agent is Claude Code in print mode (`claude -p`, claude-opus-5-5),
started in an empty directory outside the repository:

```sh
claude -p "$(cat run-T1.prompt.txt)" --model claude-opus-5-5 \
  --tools "" --setting-sources project \
  --strict-mcp-config --mcp-config mcp.json \
  --allowedTools mcp__lake \
  --no-session-persistence --output-format stream-json --verbose < /dev/null
```

- **`--tools ""`** gives it no built-in tool: no shell, no file.
- **`--strict-mcp-config`** gives it exactly one MCP server, the lake,
  as the principal `analyst`:

```json
{"mcpServers": {"lake": {"command": "lakelands",
  "args": ["--root", "<walk>", "--as", "analyst", "serve", "--mcp"]}}}
```

- **`--setting-sources project`** is new beside walk 5. Without it the
  session loaded the walker's own user-level instructions, and a
  trial session showed them to the agent. The empty directory has no
  project settings, so the agent reads no instructions but the prompt.

## The two doors

| Condition | The door | Built from |
|---|---|---|
| **T** | the MCP door as develop lists it: 20 tools, the graph tool is `query` | develop at e6b9164e |
| **C** | the same 20 tools with `query` replaced by `cypher` | branch `walk/6-cypher-tool` at cf48702570b46db0af55fdbbdc4d074402407d76 |

The branch holds one commit beside develop and is never merged. It
declares the tool `cypher` (`statement`, `params`, `cursor`, `as_of`,
`valid_at`) in `crates/lakelands-api/src/tools.rs`, served by the
`Door::Cypher` handler the REST route already uses, and pins its reads
in `crates/lakelands-server/src/session.rs` as `query`'s are. Its
description, written for the walk, states the subset as `docs/rest.md`
does; it is in `walk-6/tools-C.json`. The other 19 tools are
identical, word for word, in both lists.

`docs/walks/walk-6/` holds the record:

| File | What it holds |
|---|---|
| `run-<R>.prompt.txt` | the prompt of run R, word for word; the four are one prompt, `prompt.txt` |
| `run-<R>.jsonl` | every message of run R, stripped as walk 5's are: no harness lines, no session, message or request ids, no thinking signatures and no empty thinking, no usage. The paths of the walk's scratch directory read `<walk>` |
| `run-<R>.txt` | the same, rendered by `docs/walks/walk-5/render.py` |
| `tools-T.json`, `tools-C.json` | the tool list the analyst sees on each door, with every description and schema |

## The materials

| File | What it holds |
|---|---|
| `walk-6/lake/types/` | five types of a trading house: `supplier`, `product` (`supplied_by` a supplier), `customer`, `order` (`placed_by` a customer) and `order_line` (`of_order` an order, `of_product` a product). Each relation is a reference field and declares an inverse: `products`, `orders`, `lines`, `lines` |
| `walk-6/lake/make_data.py` | with `write DIR`, the data; with no argument, every task's reference answer |
| `walk-6/lake/principal.json` | the principal `analyst` (kind `agent`), who may read everything and write nothing |
| `walk-6/setup.sh` | a fresh lake from these, in an empty directory |

The data is twelve orders of six named customers, which the tasks ask
about, in a trade of 6,121 records drawn with a fixed seed: 12
suppliers, 81 products, 60 customers, 1,500 orders and 4,468 order
lines. No drawn record links to a named one, so the named records'
answers do not move with the seed.

| File | sha256 |
|---|---|
| `customers.ndjson` | `c2f0d7b3963ee49c74ff7891bf615964b9e05a5c314af9524f88b953fb0786bd` |
| `order_lines.ndjson` | `2c0a54ad6292c560711251f50f928a9fabb6c39b1bd017ad3febf8fc2181baae` |
| `orders.ndjson` | `fb8be2915597fac5f5a8fd912ec0b1ed9aeef7e4a2d6b7e9f7180e18ab9b8c50` |
| `products.ndjson` | `10d27a20ac8697f15cb324f362c7c8b217bebc6f1f888becc8b897834d1d68d4` |
| `suppliers.ndjson` | `301b1350e89c462b91faf06a36047ed4e57d06254c9251e0c68b17b98b95c218` |

## The commands

From a fresh clone, in the repository's root, with `lakelands` built
from the condition's tree:

```sh
docs/walks/walk-6/setup.sh <dir>
```

It runs `lakelands init walk`, `lake new trade`, `apply trade` (`applied
5 declaration(s), in force from position 8`), and one `ingest … --source
erp` per data file, the last committing through position 6134. It then
adds `principal.json` to `walk/lakelands.json`. Every run's `pin`
answered `{"pinned":6135,"position":6135}`, and every answer of every run
that names a position names 6135.

## A pilot, and why the lake is large

A first lake held the named records alone, 60 of them, and the four
runs on it read every type whole in their first calls, one page each.
They could then answer any task from what they had read, and called
the graph tool 3 to 8 times a run: the walk would have measured the
agent's memory as much as the doors. The lake was grown to what no page
holds, and the tasks and their reference answers were fixed again
before any run of this walk. Two tasks changed: the pilot's task 5
asked for the orders and their total per customer, and now asks per
country; task 10 is new. The
others are the pilot's, word for word. The grown lake, its tasks and
their answers were committed at cd43558e (07:19:39 UTC), before the
first run began (07:19:57 UTC). The pilot's four transcripts, stripped
as these are, were committed with them and are replaced by the runs
below; they stay in the history at cd43558e. Two things it showed recur
below: the Cypher door's refusal of `ORDER BY` beside an aggregate, and
the agent reading the small types whole.

## The tasks

| # | The question, as the prompt puts it | What it covers | Reference answer |
|---|---|---|---|
| 1 | Which orders has the customer Harbor Supplies placed? Give each order's number, date, total and status. | neighbours | O-1002, 2026-08-11, 372.00, shipped; O-1006, 2026-09-09, 142.50, open; O-1011, 2026-10-13, 144.00, cancelled |
| 2 | Which products are on order O-1007, and how many units of each? | two hops | Check valve DN25 (P-21) × 2; O-ring set (P-40) × 8 |
| 3 | Which suppliers make the products on order O-1004, and in which countries are they? | three hops | Nordic Fasteners (S-1), SE; Lago Valvole (S-2), IT; Torino Guarnizioni (S-4), IT |
| 4 | Which open orders contain at least one valve (a product in the category `valves`)? | a filter on a related record | O-1004, O-1007, O-1009 |
| 5 | For each country, how many orders have its customers placed, and what do those orders come to in total? Count every order, whatever its status. | a count and a sum, grouped by a related record's field | AT 26, 146596.80; CZ 106, 675799.74; DE 186, 936721.36; DK 213, 1196491.38; FR 117, 661889.91; GB 173, 905960.42; IE 159, 820741.12; NL 119, 658280.78; PL 251, 1355493.77; PT 150, 833752.68 |
| 6 | Leaving out cancelled orders, how many units of each product category have been ordered? | a group over two steps | bearings 28842, cables 27468, fasteners 30371, fittings 26778, pumps 14, seals 25422, tools 27576, valves 25 |
| 7 | Rank all order lines by quantity, highest first, and give me ranks 6 to 10: each line's id, its order, its product and its quantity. Read the ranking from the store at most five lines at a time. | ordering, and paging past the first page | 6. L-1012-2, O-1012, P-12, 300; 7. L-1006-2, O-1006, P-11, 250; 8. L-1006-1, O-1006, P-10, 200; 9. L-1004-4, O-1004, P-10, 150; 10. L-1012-1, O-1012, P-10, 90 |
| 8 | Kestrel Farms and Ostrava Machining: how are they connected in the data? Give the shortest chain of records that links them. | a path, which the subset cannot spell | the one chain of 6 links: C-4 ← O-1012 ← L-1012-2 → P-12 (Washer M8) ← L-1008-1 → O-1008 → C-5; both bought washers |
| 9 | Which products have never been ordered? | a step that must not match | P-13 Threaded rod M8, P-42 Shaft seal 30mm, P-167, P-168, P-169 |
| 10 | Which customers in Germany have bought at least one product made in Italy? | four hops, the same customer on many rows. Only the filter on Germany is tested: all 8 German customers bought something Italian, so an answer that left out the filter on Italy would be the same | C-1 Delta Metals, C-8, C-20, C-39, C-40, C-53, C-55, C-59 (135 rows before they are told apart) |

The reference answers were fixed before any run: `lake/make_data.py`
prints them from the lists it writes the data from, not from the lake.

## How a run is counted

Each task is counted from the run's own calls, numbered as `run-<R>.txt`
numbers them.

- **First try:** the first call the agent made for the task answered
  it, without a refusal, and the agent's final answer is the reference
  answer.
- **Right:** the agent's final answer is the reference answer.
- **Calls:** every call the agent made for the task, its checks
  included. Tasks 2 and 3 share a call where the agent read both
  orders' lines at once; it counts for task 2.
- **Refusals:** each refused call, and whether it was understood.

Every run began with the same seven calls: `describe`, `pin`, one page
each of all customers, suppliers and products, and a count of orders
and of order lines. They are counted apart, as set-up. Tasks 1, 2, 3
and 7 were answered with the `<type>_find` tools in every run, on both
doors; the doors differ on tasks 4, 5, 6, 8, 9 and 10, which are marked
**G** below.

## Condition T: `query`

| Task | T1 first try | T1 right | T1 calls | T1 refusals | T2 first try | T2 right | T2 calls | T2 refusals |
|---|---|---|---|---|---|---|---|---|
| set-up | | | 7 (1–7) | | | | 7 (1–7) | |
| 1 | yes | yes | 1 (8) | | yes | yes | 1 (8) | |
| 2 | yes | yes | 1 (9) | | yes | yes | 1 (9) | |
| 3 | yes | yes | 0 (9) | | yes | yes | 0 (9) | |
| 4 **G** | yes | yes | 3 (10–12) | | no | yes | 4 (10–13) | 10, understood |
| 5 **G** | no | yes | 3 (13–15) | 13, understood | no | yes | 4 (14–17) | 14 and 15, both understood |
| 6 **G** | yes | yes | 2 (16–17) | | yes | yes | 2 (18–19) | |
| 7 | no | yes | 4 (18–21) | 19, understood | no | yes | 4 (20–23) | 21, understood |
| 8 **G** | no | yes | 2 (22–23) | 22, understood | no | yes | 2 (24–25) | 24, understood |
| 9 **G** | yes | yes | 3 (24–26) | | yes | yes | 2 (26–27) | |
| 10 **G** | yes | yes | 2 (27–28) | | yes | yes | 2 (28–29) | |

The refusals:

- **T1 13, T2 15:** an aggregate written with `project.fields`: "there
  is no member `fields`; it takes `measures`, `group_by`". The schema
  lists `fields` and neither of the others (lakelands#1443).
- **T1 22, T2 24:** a step's `depth` written `{"max": 8}`: "the member
  `min` is missing". The schema requires neither member (lakelands#1444).
- **T1 19, T2 21:** the second page asked with the cursor alone: "the
  cursor was handed out for another order; continue it with the
  `order_by` its first page was read with" (lakelands#1445).
- **T2 10:** a step named by the relation's inverse, `lines`, from
  `order`: "`order` has no relation `lines` to walk in". The next call
  walked `of_order` `in`.
- **T2 14:** `order_find` grouped by `customer.country`, through the
  reference: "a condition reads the record's own fields. Compare the
  reference's key, or walk to the record it names in a pattern". The
  next call walked to the customer in `query`.

## Condition C: `cypher`

| Task | C1 first try | C1 right | C1 calls | C1 refusals | C2 first try | C2 right | C2 calls | C2 refusals |
|---|---|---|---|---|---|---|---|---|
| set-up | | | 7 (1–7) | | | | 7 (1–7) | |
| 1 | yes | yes | 1 (8) | | yes | yes | 1 (8) | |
| 2 | yes | yes | 1 (9) | | yes | yes | 1 (9) | |
| 3 | yes | yes | 1 (10) | | yes | yes | 0 (9) | |
| 4 **G** | yes | yes | 4 (11–14) | 13, understood | yes | yes | 2 (10–11) | |
| 5 **G** | no | yes | 2 (15–16) | 15, understood | no | yes | 3 (12–14) | 12, understood |
| 6 **G** | yes | yes | 2 (17–18) | | yes | yes | 2 (15–16) | |
| 7 | no | yes | 4 (19–22) | 20, understood | no | **no** | 3 (17–19) | |
| 8 **G** | yes | yes | 1 (23) | | yes | yes | 1 (20) | |
| 9 **G** | yes | yes | 2 (24–25) | | yes | yes | 2 (21–22) | |
| 10 **G** | yes | yes | 1 (26) | | yes | yes | 1 (23) | |

The refusals:

- **C1 15, C2 12:** `RETURN c.country, count(*) AS orders, sum(o.total)
  AS total ORDER BY c.country`: "`order_by` orders rows; a graph and a
  path come by depth, type and key, and an aggregate's groups by their
  key". The statement has no `order_by`; the refusal is the pattern's,
  in the pattern's words (lakelands#1446). Both next calls dropped
  `ORDER BY`. Fixed by lakelands#1451: the subset itself now refuses
  `ORDER BY` beside an aggregate, as `order_by_aggregate`, at `ORDER`'s
  character offset.
- **C1 20:** the cursor alone, as in T (lakelands#1445).
- **C1 13:** a check after task 4 was already answered: `order_find`
  with `product in ['P-20', 'P-21']`, "does not parse: expected `(`".
  A statement takes `IN [...]`; the `where` of a find takes
  `in (...)`. The next call wrote two `or`ed comparisons.

**C2's task 7** is the one wrong answer of the walk. The store answered
rank 10 as `L-1012-1`, `O-1012`, `P-10`, 90; the agent's final answer
gives its product as `P-12`. The read was a `<type>_find`, the same on
both doors, and the store's answer was right: the agent copied it
wrong.

## The sums

| | T1 | T2 | C1 | C2 |
|---|---|---|---|---|
| right on the first try, of 10 | 7 | 6 | 8 | 8 |
| right at all, of 10 | 10 | 10 | 10 | 9 |
| calls, of them on the tasks | 28, 21 | 29, 22 | 26, 19 | 23, 16 |
| refusals, of them understood | 3, 3 | 5, 5 | 3, 3 | 1, 1 |
| on the graph tasks **G**: right on the first try, of 6 | 4 | 3 | 5 | 5 |
| calls of the graph tool, of them refused | 11, 2 | 11, 3 | 8, 1 | 8, 1 |
| characters the agent wrote to the graph tool | 3,786 | 3,519 | 1,507 | 1,449 |
| characters the graph tool answered | 6,847 | 7,494 | 17,467 | 17,467 |

By condition: on the graph tasks, T was right on the first try 7 times
of 12 and C 10 times of 12; both were right at all 12 times. T's graph
tool was refused 5 times in 22 calls, C's 2 times in 16. Every refusal
of the walk was understood: each next call changed the right thing.

## What the runs show

- **Counted the same way on both doors, C stays ahead.** As the
  doors stood, T was right on the first try 7 times of 12 on the graph
  tasks and C 10 times. Some losses on each door come from a sentence
  its own tool's text lacks:
  - **T:** three, lakelands#1443 or lakelands#1444 alone (T1's tasks 5
    and 8, T2's task 8). The agent wrote what the schema showed, and
    the refusal told it the rest.
  - **C:** two, task 5 in both runs. The `cypher` description, written
    for this walk, says "`ORDER BY` names fields" and nothing of an
    aggregate, beside which `ORDER BY` is refused (its refusal's wording
    was lakelands#1446, fixed by lakelands#1451).
  - With every such sentence written, T would have stood at 6 and 4 of
    6, 10 of 12, and C at 6 and 6, 12 of 12. T's two remaining losses
    are both T2's: the inverse name on task 4, and task 5, which began
    with a find's group key through a reference (T2 14) before it met
    lakelands#1443. These counts are inferred from which refusal cost
    which try; no run was made with the texts fixed.
- **The path is the store's in T and the agent's in C.** In T, both
  runs asked for the shortest walk with `to`, and the store found it.
  The subset cannot spell a path. In C, both runs wrote the one chain
  they expected, customer ← order ← line → product ← line → order →
  customer, as a fixed pattern, and argued from `describe` that nothing
  shorter could exist. The argument holds on this schema; on a schema
  with a shorter way round, the subset would have answered the chain
  the agent guessed, not the shortest.
- **C writes less and reads more.** A statement is a half to two
  thirds as long as the pattern T wrote for the same task (task 6: 190
  and 180 characters, against 401 and 361). But the subset has no `DISTINCT`, and
  for task 10 both C runs read the 135 matching rows (14,158
  characters) and told the eight customers apart themselves, where
  both T runs grouped by customer and read 8 groups. Grouping
  (`count(*)` beside `c.id`) was open to C too; neither run used it.
- **Two expression spellings.** On door C the agent meets the
  statement's `IN [...]` and the find tools' `in (...)` in one session
  (C1 13).
- **Paging through `cypher` was not measured.** Task 7, the paging
  task, was answered with `order_line_find` in all four runs, so no run
  sent a statement's `next_cursor` back. Neither was a door with both
  `query` and `cypher`: C had `cypher` in `query`'s place.
- **Task 10 tests one of its two filters.** Every German customer
  bought something made in Italy, so a reading that left out the
  filter on Italy would have been counted right. All four runs wrote
  both filters (T1 27, T2 28, C1 26, C2 23), so no count above is
  flattered by it; the 135 rows C read are the rows both filters
  keep.
- **The rest is the same on both doors.** Every run read the
  customers, suppliers and products whole in its set-up, and answered
  tasks 1, 2, 3 and 7 with the find tools. The condition reaches six
  tasks of ten.

## Found on the walk

| Runs and calls | Finding | Issue |
|---|---|---|
| T1 13, T2 15 | `query`'s `project` schema shows `fields` but not an aggregate's `group_by` and `measures` | lakelands#1443, fixed by lakelands#1505 |
| T1 22, T2 24 | A step's `depth` needs `min` and `max`, and `query`'s schema requires neither | lakelands#1444, fixed by lakelands#1505 |
| T1 19, T2 21, C1 20 | A find's `cursor` has no description; three of four runs sent it without its `order_by` | lakelands#1445, fixed by lakelands#1505 |
| C1 15, C2 12 | Cypher's `ORDER BY` beside an aggregate is refused in the pattern's words, as `order_by`, with no offset | lakelands#1446, fixed by lakelands#1451 |

**Observations, with no issue:**

- **An inverse name in a step (T2 10).** `describe` lists each
  relation's inverse beside it; a step takes only the declared name.
  The refusal says that `order` has no relation `lines`, and the agent
  found `of_order` at once. It could name `of_order` itself.
- **A group key through a reference (T2 14).** The refusal speaks of
  "a condition" for a group key, but says what to do, and the agent did
  it.
- **The `where` of a find takes `in (...)` (C1 13).** Its refusal names
  the parenthesis it expected; the agent wrote `or` instead.

## Recommendation

**Yes, as a read tool beside `query`, not in its place.** This is a
recommendation; the decision is the project's.

- **The measured reason.** On the six graph tasks, the agent on `cypher`
  was right on the first try 10 times of 12 against 7 on `query`, and
  was refused 2 times in 16 graph calls against 5 in 22. Counted the
  same way on both doors, with each tool's missing sentences written,
  the lead is 12 of 12 against 10 of 12. Right at all, the doors stood
  level, 12 of 12 each, so the lead is in tries and calls, not in
  answers. A statement was a half to two thirds as long as the pattern
  for the same task.
- **Not in `query`'s place, measured.** The subset cannot ask for the
  shortest walk. On task 8, both C runs answered with the chain they
  guessed and argued that nothing shorter could exist; both T runs had
  the store find it with `to`. The argument held on this schema only.
- **With the description's sentence.** The `cypher` description should
  say that `ORDER BY` beside an aggregate is refused, and
  lakelands#1446 should word the refusal in the statement's terms
  (fixed by lakelands#1451: the subset refuses `ORDER BY` beside an
  aggregate at `ORDER`'s offset); both C losses were there.
- **Argued, not measured.**
  - Paging through `cypher` and a door listing both tools: no run used
    either.
  - The cost of a second expression spelling: one refusal in two runs
    (C1 13) measures it only once.
  - Two runs per door is a small sample. The lead after the fixes is
    T2's two losses alone; T1 would have matched C.

What would change it: the same tasks on a door with both tools, with
the `cypher` sentence fixed (lakelands#1443, lakelands#1444 and
lakelands#1445 are, by lakelands#1505), and a
paging task the agent must answer through `cypher`. If `query` then
wins the first tries that `cypher` wins here, the lead was the texts,
not the language.

## Recording the walk

The runs were made on 2026-10-26: T against develop at e6b9164e, C
against `walk/6-cypher-tool` at
cf48702570b46db0af55fdbbdc4d074402407d76, each run on a fresh lake from
`setup.sh`. The four ran at the same time, from 07:19 to 07:22 UTC.
The agent's answers were compared with the reference answers above by
hand, field by field.
