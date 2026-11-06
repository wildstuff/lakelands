# Walk 7: the MCP door with `query` and `cypher` side by side

Walk 6 (`docs/walks/walk-6.md`) measured `query` and `cypher` on two
doors, each tool in the other's place, and recommended `cypher` as a
read tool beside `query`. The door now lists both. This walk asks of
that door:

> **Does the MCP door with `query` and `cypher` side by side tell an
> agent how to find the data, and does the agent pick the tool that
> answers a task?**

The walker is walk 6's blind agent: a fresh LLM session whose only
tools are a lake's MCP tools. It ran three times, each run on a fresh
lake: twice on the door as develop lists it, and once, after the
findings below were fixed, on the fixed door. The answers below are
taken from its transcripts as they happened. The walk is a
measurement, not a contract: no test replays it.

A refusal or a tool description that told the agent less than it
needed, or something wrong, is a **finding**. A refusal is
**understood** when the agent's next call changed the right thing.

## The agent

The agent is walk 6's, unchanged: Claude Code in print mode (`claude
-p`, claude-opus-5-5), started in an empty directory outside the
repository, with no built-in tool (`--tools ""`), no instructions but
the prompt (`--setting-sources project`) and exactly one MCP server,
the lake, as the principal `analyst` (`--strict-mcp-config`):

```sh
claude -p "$(cat run-1.prompt.txt)" --model claude-opus-5-5 \
  --tools "" --setting-sources project \
  --strict-mcp-config --mcp-config mcp.json \
  --allowedTools mcp__lake \
  --no-session-persistence --output-format stream-json --verbose < /dev/null
```

## The door

| Runs | The door | Built from |
|---|---|---|
| 1, 2 | the MCP door as develop lists it: 21 tools, `query` and `cypher` among them | develop at 370aebf6 |
| 3 | the same 21 tools, with the three texts of § Found on the walk fixed | branch `docs/1460-walk-7` at 6b6f41aa |

The 21 tools are walk 6's 20 of door T and `cypher`. Between the two
doors only the three schema texts of the findings differ; every name,
every other description and every schema is the same, word for word
(`walk-7/tools-1.json` against `walk-7/tools-3.json`).

`docs/walks/walk-7/` holds the record:

| File | What it holds |
|---|---|
| `prompt.txt` | the prompt, word for word: walk 6's ten tasks and seven new ones |
| `answers.py` | the reference answers of tasks 11 to 17, computed from walk 6's `make_data.py` lists and not from the lake |
| `run.sh` | one run: a fresh lake from `walk-6/setup.sh` in a scratch directory, the analyst's tool list, and the agent on the prompt |
| `tools.py` | the analyst's tool list over stdio (`initialize`, `tools/list`) |
| `strip.py` | strips a run's stream as walk 6's are: no harness lines, no session, message or request ids, no thinking signatures and no empty thinking, no usage; the scratch directory reads `<walk>` |
| `run-<R>.prompt.txt` | the prompt of run R; the three are `prompt.txt` |
| `run-<R>.jsonl` | every message of run R, stripped |
| `run-<R>.txt` | the same, rendered by `docs/walks/walk-5/render.py` |
| `tools-<R>.json` | the tool list the analyst saw in run R, with every description and schema |

## The lake and the commands

The lake is walk 6's, unchanged: `walk-6/lake/` and `walk-6/setup.sh`,
the twelve named orders of six named customers in a trade of 6,121
records. From the repository's root, with `lakelands` built from the
door's tree:

```sh
docs/walks/walk-7/run.sh <scratch> <run> <lakelands>
```

Every run's `pin` answered `{"pinned":6136,"position":6136}`, one
position past walk 6's 6135 on the same records, and every answer of
every run that names a position names 6136.

## The tasks

Tasks 1 to 10 are walk 6's, word for word, with walk 6's reference
answers (`walk-6.md` § The tasks). The plan asked for twelve; walk 6
has ten, and the ten are kept. Tasks 11 to 17 are new:

| # | The question, as the prompt puts it | What it covers | Reference answer |
|---|---|---|---|
| 11 | Kestrel Farms buys from us. Who makes the goods they have bought the most units of, and in which country is that maker? | discovery: a customer and a maker named in plain words | Nordic Fasteners (S-1), SE: 390 units (Torino Guarnizioni 8, Lago Valvole 2) |
| 12 | Which of our customers have ever bought anything made by Rhein Pumpen? | discovery, three hops back from a maker | C-1 Delta Metals, C-3 Brightwater Utilities, C-5 Ostrava Machining |
| 13 | What has Delta Metals ordered from us altogether, in money, and which of their orders are still waiting to go out? | discovery: a sum and a status in plain words | 1653.00 (O-1001 shipped 340.00, O-1004 open 1028.00, O-1010 open 285.00); waiting: O-1004 and O-1010 |
| 14 | List every order line of a product made by Lago Valvole, largest quantity first: each line's id, its order, its product and its quantity. Read the lines with the `cypher` tool, at most 2 lines a call. | paging through `cypher` | L-1004-1 O-1004 P-20 10; L-1002-1 O-1002 P-20 6; L-1009-2 O-1009 P-20 4; L-1003-3 O-1003 P-21 3; L-1007-1 O-1007 P-21 2 |
| 15 | Which orders contain goods made by Torino Guarnizioni? Give each order's number, its customer and its status, in order of the order number. Read them with the `query` tool, at most 2 rows a call. | paging through `query` | O-1002 C-2 shipped; O-1004 C-1 open; O-1007 C-4 open; O-1009 C-3 open; O-1011 C-2 cancelled |
| 16 | Harbor Supplies and Rhein Pumpen: how are they connected in the data? Give the shortest chain of records that links them. | tool choice: a path, which only `query` can ask for | 8 links, three such chains, e.g. C-2 – O-1002 – L-1002-1 – P-20 – L-1009-2 – O-1009 – L-1009-1 – P-30 – S-3 |
| 17 | Which open orders placed by customers in Ireland are worth more than 12,000.00? Give each order's number, its customer and its total. | tool choice: plainest as a statement | O-1125 C-47 12577.27; O-1161 C-11 13905.46; O-1243 C-47 15852.97; O-1820 C-43 16538.06; O-2209 C-17 12508.70 |

The tasks, their reference answers and the runner were committed at
732bf184 (16:33:04 UTC), before the first run began (16:34:42 UTC).

## How a run is counted

As walk 6 counts it. Each task is counted from the run's own calls,
numbered as `run-<R>.txt` numbers them.

- **First try:** the first call the agent made for the task answered
  it, without a refusal, and the agent's final answer is the reference
  answer.
- **Right:** the agent's final answer is the reference answer.
- **Calls:** every call the agent made for the task, its checks
  included. Where one call answered two tasks it counts for the first.
- **Refusals:** each refused call, and whether it was understood.
- **Tool:** the tool whose answer the final answer was read from.

Every run began with the same seven calls: `describe`, `pin`, one page
each of all customers, suppliers and products, and a count of orders
and of order lines, counted apart as set-up. Runs 1 and 3 ended with
`pin` `{"clear": true}`, counted apart too. Walk 6's graph tasks (4, 5,
6, 8, 9 and 10) are marked **G**; beside them stand walk 6's first
tries, T (on `query`) and C (on `cypher`), two runs each.

## The runs

| Task | Tool | R1 first try | R1 calls | R1 refusals | R2 first try | R2 calls | R2 refusals | R3 first try | R3 calls | Walk 6 T1 T2 C1 C2 |
|---|---|---|---|---|---|---|---|---|---|---|
| set-up | | | 7 (1–7) | | | 7 (1–7) | | | 7 (1–7) | |
| 1 | find | yes | 1 (8) | | yes | 1 (8) | | yes | 1 (8) | y y y y |
| 2 | find | yes | 1 (9) | | yes | 1 (9) | | yes | 1 (9) | y y y y |
| 3 | find | yes | 1 (10) | | yes | 0 (9) | | yes | 1 (10) | y y y y |
| 4 **G** | cypher | yes | 2 (11–12) | | yes | 2 (10–11) | | yes | 2 (11–12) | y n y y |
| 5 **G** | cypher | yes | 1 (13) | | yes | 1 (12) | | yes | 1 (13) | n n n n |
| 6 **G** | cypher | yes | 2 (14–15) | | yes | 3 (13–15) | | yes | 2 (14–15) | y y y y |
| 7 | find | no | 4 (16–19) | 17, understood | yes | 3 (16–18) | | yes | 3 (16–18) | n n n n |
| 8 **G** | query | no | 2 (20–21) | 20, understood | no | 3 (19–21) | 19 and 20, understood | yes | 1 (19) | n n y y |
| 9 **G** | cypher | yes | 3 (22–24) | 23, understood | yes | 1 (22) | | yes | 2 (20–21) | y y y y |
| 10 **G** | cypher | yes | 2 (25–26) | | yes | 1 (23) | | yes | 1 (22) | y y y y |
| 11 | cypher | yes | 1 (27) | | yes | 1 (24) | | yes | 2 (23–24) | |
| 12 | cypher | yes | 1 (28) | | yes | 1 (25) | | yes | 1 (25) | |
| 13 | find | yes | 1 (29) | | yes | 2 (26–27) | | yes | 2 (26–27) | |
| 14 | cypher | yes | 3 (30–32) | | yes | 3 (28–30) | | yes | 3 (28–30) | |
| 15 | query | yes | 4 (33–36) | | yes | 4 (31–34) | | yes | 3 (31–33) | |
| 16 | query | yes | 1 (37) | | yes | 1 (35) | | yes | 1 (34) | |
| 17 | cypher | yes | 3 (38–40) | | yes | 3 (36–38) | | yes | 2 (35–36) | |

Every run gave the reference answer to all 17 tasks. The tool was the
same in all three runs for every task.

The refusals, all on the door of runs 1 and 2:

- **R1 17:** the second page of task 7 asked with the cursor and
  `limit` alone: "the cursor was handed out for another order; continue
  it with the `order_by` its first page was read with". The next call
  sent the first page's `order_by` with it. Walk 6 met it in three runs
  of four (lakelands#1445).
- **R1 20, R2 19:** a step's `depth` written `{"max": 10}` and
  `{"max": 8}`: "the member `min` is missing". Walk 6 met it in both
  runs of T (lakelands#1444). R1's next call gave every step `{"min": 0,
  "max": 8}` and was answered.
- **R2 20:** R2's next call gave every step `{"min": 1, "max": 8}`, "so
  they form one combined walk": "step 1 over `placed_by`: it reaches
  several types, `customer`, `order`, and the step after it reads one;
  name the step's `type`". A step with `min` 1 makes the steps a chain,
  and the refusal is a chain's. The agent did not name a type, as the
  refusal says, but set `min` 0, from the `query` description ("a graph
  whose steps all start at depth 0 is one walk"), and was answered.
  Understood by the rule above: the next call changed the right thing;
  but not from the refusal's words.
- **R1 23:** a check after task 9 was already answered: `order_line_find`
  with `product in ['P-13', …]`, "does not parse: expected `(`". The
  next call wrote `in (...)`. Walk 6 met it once (C1 13).

## The sums

| | R1 | R2 | R3 | walk 6, T1 T2 C1 C2 |
|---|---|---|---|---|
| right on the first try, of 17 | 15 | 16 | 17 | |
| right at all, of 17 | 17 | 17 | 17 | |
| on tasks 1–10, right on the first try | 8 | 9 | 10 | 7, 6, 8, 8 |
| on the graph tasks **G**, right on the first try, of 6 | 5 | 5 | 6 | 4, 3, 5, 5 |
| calls, of them on the tasks | 41, 33 | 38, 31 | 37, 29 | |
| refusals, of them understood | 3, 3 | 2, 2 | 0 | |
| `query` calls, of them refused | 6, 1 | 7, 2 | 5, 0 | |
| `cypher` calls, of them refused | 13, 0 | 12, 0 | 12, 0 | |
| characters the agent wrote to `query` and `cypher` | 6,260 | 6,580 | 5,957 | |
| characters `query` and `cypher` answered | 12,629 | 13,698 | 12,665 | |

On the graph tasks, the door with both tools was right on the first try
10 times of 12 in runs 1 and 2, as door C was in walk 6 and against door
T's 7. Both losses are task 8's `depth`, lakelands#1444. On the fixed
door, run 3 was right on the first try on every task, with no refusal.

## What the runs show

- **The agent picks the tool by the task, and the same way every
  time.** Graph reads with a filter, a group or a sum went to `cypher`
  (tasks 4, 5, 6, 9, 10, 11, 12, 17); the two paths went to `query` with
  `to` (8 and 16); a type read by its own fields went to a find (1, 2,
  3, 7, 13). The choice did not vary between runs on any of the 17
  tasks. Task 16, which only `query` can answer, went to `query` in all
  three runs, and its first call found the shortest chain. Task 17,
  plainest as a statement, went to `cypher` in all three, one
  statement each.
- **Walk 6's `cypher` loss is gone.** Walk 6's door C lost task 5 in
  both runs to `ORDER BY` beside an aggregate (lakelands#1446). Here
  every run wrote task 5's statement without `ORDER BY`, and the
  `cypher` description now says why; no `cypher` call of any run was
  refused.
- **Neither was `query`'s aggregate schema met.** Every aggregate went
  through `cypher` or a find's `aggregate`, so no run wrote `query`'s
  `project` for one, and lakelands#1443 is not measured here.
- **Paging works through both tools.** Task 14 was read through
  `cypher` in three pages of at most two lines in every run, each
  `next_cursor` sent back with the same statement; task 15 through
  `query` in three pages of two rows. No paged call of either tool was
  refused, and every run joined the pages to the reference answer. Task
  15's rows are one per line, and every run told O-1004's two lines
  apart into one order.
- **The discovery tasks were answered from the set-up.** Every run
  read the customers, suppliers and products whole in its first calls
  (60, 12 and 81 records, one page each), and found Kestrel Farms,
  Rhein Pumpen, Delta Metals, Lago Valvole and Torino Guarnizioni in
  what it had read. Run 2 named the maker in task 14's statement by its name
  (`supplier {name: $s}`, `$s` = `Lago Valvole`); the others used the
  key read in the set-up. Whether the door tells an agent how to find a record by
  its name in a type no page holds is not measured.
- **Every run checked.** Each checked sums against each other (task
  5's countries against the totals by status, task 6's categories
  against the units by status) and task 17's orders a second way,
  through `order_find`, and each read the aging rules
  (`sys_aging_rule_find`) before answering. Every check agreed.

## Found on the walk

Each finding below is a text, fixed in this walk's change, with the
text as it was, what the agent did, and the text as it is now. A
regression test (`crates/lakelands-api/src/tools/tests.rs`,
`a_cursor_a_list_and_a_depth_say_what_their_refusals_ask`) holds the
three. No finding was a defect of the code: every refusal was right,
and none is filed as an issue.

| Runs and calls | Finding | Issue |
|---|---|---|
| R1 17; walk 6 T1 19, T2 21, C1 20 | A find's `cursor` has no description, and the agent sends it without the first page's `order_by` | the text half of lakelands#1445 |
| R1 20, R2 19; walk 6 T1 22, T2 24 | A step's `depth` needs both members, and its schema says nothing | the text half of lakelands#1444; the schema's `required` stays there |
| R2 20 | `min` 1 makes the steps of a path a chain, and nothing beside `depth` says so; the chain's refusal then points at a step's `type` | |
| R1 23; walk 6 C1 13 | A find's `where` takes a list in parentheses, a statement in brackets, and the agent meets both in one session | |

**A find's `cursor`** (every `<type>_find`, and every `sys_*_find`):

- Before: no description, `{"type": "string"}`.
- After: "The `next_cursor` of the page before, to read the next page:
  give it with the same `where` and `order_by` the first page was read
  with."

**A step's `depth`** (`query`, `steps[].depth`):

- Before: no description, an object of `min` and `max`.
- After: "How many hops the step walks, both members given, as
  {"min": 0, "max": 6}. A graph or a path (`to`) whose steps all have
  `min` 0 is one walk over their relations, in any order; a step with
  `min` 1 or more makes it a chain, each step walking on from where the
  one before it stopped."

**A find's `where`** (every `<type>_find`, and every `sys_*_find`):

- Before: "A condition in the expression grammar, e.g. status = 'open'
  and amount > 100."
- After: "A condition in the expression grammar, e.g. status = 'open'
  and amount > 100; a list is written in parentheses, status in
  ('open', 'paid'), not in brackets as a `cypher` statement writes it."

The refusals themselves are unchanged. The chain's refusal (R2 20) is
right for a chain and for rows, where naming the step's type is the
answer; the agent's mistake was earlier, in writing a chain when it
meant one walk, and the `depth` text is where that is decided. The
other three refusals were understood at once.

The server's MCP transcripts
(`crates/lakelands-server/tests/transcripts/`) were re-recorded for
the new texts. `docs/hosting.md` § Rules the host inherits names the
one tool surface and no text a finding names, and is unchanged.

The door checks (`crates/lakelands-door-checks`, run over MCP by
`crates/lakelands-client/tests/door_checks.rs`) hold the two `cypher`
paths of this walk: a record named in plain words found through
`describe` and a `cypher` statement, read page by page
(`a_record_named_in_plain_words_is_found_through_describe_and_cypher`),
beside the paged statement already there
(`the_tool_pages_with_the_cursor_beside_the_statement`).

## What was not measured

- **Discovery on a type no page holds.** The named records were found
  in the set-up's whole reads; no run had to find a record by its name
  through `search` or a `where` on a type too large to read.
- **`query`'s aggregate.** No run wrote one (lakelands#1443).
- **Whether the fixed texts help another agent.** Run 3 is one run on
  the fixed door, by the same agent as runs 1 and 2. Its 17 first tries
  show that the texts were read and enough for this agent on these
  tasks, not more.
- **Three runs is a small sample.** Runs 1 and 2 differ on task 7 (R2
  sent the cursor with its `order_by` unprompted) and on task 8 (R2
  took two refusals to R1's one); either could fall the other way.

## Recording the walk

The runs were made on 2026-10-26, each on a fresh lake from
`walk-6/setup.sh`: runs 1 and 2 against develop at 370aebf6, at the
same time, from 16:34 to 16:37 UTC; run 3 against 6b6f41aa, from 16:48
to 16:50 UTC. The agent's answers were compared with the reference
answers above by hand, field by field.
