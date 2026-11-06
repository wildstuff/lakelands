# Walk 8: finding data through the door in a lake too large to read whole

Walk 7 (`docs/walks/walk-7.md`) found every named record in the
set-up's whole reads: its lake held 60 customers, 12 suppliers and 81
products, one page each. It left one question open, and this walk asks
it:

> **Can an agent find data through the MCP door when the lake is too
> large to read whole, and the task names the record only in plain
> words: misspelled, shared with others, or by a word of its
> description?**

The walker is walk 7's blind agent: a fresh LLM session whose only
tools are a lake's MCP tools. It ran four times, each run on a fresh
lake: twice on the door as develop lists it, with `search` ranked by
BM25F (lakelands#1459); once after the first three findings below were
fixed; and once after the fourth, which run 3 showed, was fixed too. The answers below are taken from its transcripts as
they happened. The walk is a measurement, not a contract: no test
replays it.

A refusal or a tool description that told the agent less than it
needed, or something wrong, is a **finding**. A refusal is
**understood** when the agent's next call changed the right thing.

## The agent

The agent is walk 7's, unchanged: Claude Code in print mode (`claude
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
| 1, 2 | the MCP door as develop lists it: walk 7's 21 tools, `search` ranked by BM25F | develop at e74216d4 |
| 3 | the same 21 tools, with findings 1 to 3 of § Found on the walk fixed | branch `docs/1464-walk-8` at cb9a1081 |
| 4 | the same, with finding 4 fixed too | branch `docs/1464-walk-8` at 00f36cec |

Between the doors only the findings' schema texts differ; every name,
every other description and every schema is the same, word for word:
`walk-8/tools-1.json` and `tools-3.json` differ in the `where` of the
seven finds and the `mode`, `field` and `vector` of `search`;
`tools-3.json` and `tools-4.json` in the `text` of `search` alone.

`docs/walks/walk-8/` holds the record:

| File | What it holds |
|---|---|
| `lake/types/` | the five types: walk 6's, with `searchable` on the customer's, product's and supplier's `name`, and two new searchable texts, the product's `description` and the supplier's `notes` |
| `lake/make_data.py` | the generator: the records, drawn with a fixed seed (`random.Random(8)`), and the records the tasks name planted among them |
| `lake/principal.json` | the analyst, who reads every type |
| `setup.sh` | a fresh lake from the types and the generator's files |
| `prompt.txt` | the prompt, word for word: ten tasks |
| `answers.py` | the reference answers, computed from the generator's lists and not from the lake; task 9's ranking by a BM25F of its own, written from `docs/seams.md` |
| `run.sh` | one run: a fresh lake in a scratch directory, the analyst's tool list (walk 7's `tools.py`), the agent on the prompt, and the stream stripped (walk 7's `strip.py`) |
| `calls.py` | every call of a run: its tool, the characters written to it and answered, and whether it was refused; or, given each task's calls, their sums |
| `run-<R>.prompt.txt` | the prompt of run R; the four are `prompt.txt` |
| `run-<R>.jsonl` | every message of run R, stripped |
| `run-<R>.txt` | the same, rendered by `docs/walks/walk-5/render.py` |
| `tools-<R>.json` | the tool list the analyst saw in run R, with every description and schema |

## The lake and the commands

Walk 6's trade, grown: 300 suppliers, 2,000 products, 3,000 customers,
6,051 orders and 21,005 order lines, 32,356 records. A find answers at
most 100 records a page, so no type but the suppliers is read in three
pages, and none of the tasks' records is found by reading pages. The
customers' names are a word or two and a trade (`Echo Trading`, `Coast
Holly Supplies`), so many share a word and a few a whole name; the
products' names repeat (`Submersible pump 0.75kW` is three products),
and only their descriptions tell them apart. From the repository's
root, with `lakelands` built from the door's tree:

```sh
python3 docs/walks/walk-8/lake/make_data.py    # the counts
python3 docs/walks/walk-8/answers.py           # the reference answers
docs/walks/walk-8/run.sh <scratch> <run> <lakelands>
```

`<scratch>` is an existing empty directory. Every run's `pin` answered
`{"pinned":32371,"position":32371}`, and every answer of every run that
names a position names 32371.

## The tasks

The tasks name no type, field or key. Each is about one of the records
`make_data.py` plants; the rest of the lake is drawn around them.

| # | The question, as the prompt puts it | What it covers | Reference answer |
|---|---|---|---|
| 1 | How many orders has Kestral Farms placed with us, and what do they come to altogether, whatever their status? | a misspelled name | no Kestral; Kestrel Farms (C-687, IE): 5 orders, 92404.14 |
| 2 | Torino Guarnizoni: in which country do they make their goods, and how many of the products we sell are theirs? | a misspelled name | no Guarnizoni; Torino Guarnizioni (S-109): IT, 7 products (P-1439, P-1634, P-1700, P-2416, P-2696, P-2762, P-2805) |
| 3 | Halden Supplies, the customer in Portugal: which of our products have they bought the most units of, leaving out cancelled orders, and how many units? | a shared name told apart, then a walk and a sum | C-2113: P-1077 Relief valve DN50, 60 (P-1512 55, P-2300 28, P-1808 5) |
| 4 | When did Halden Supplies last place an order with us, and what is that order's status? | a shared name, not told apart | three customers are called so: C-258 (NL) O-11006 2026-09-30 open; C-2113 (PT) O-11833 2026-09-15 open; C-2339 (GB) O-14825 2026-10-15 cancelled |
| 5 | We sell a pump for draining flooded cellars. What is it called, who makes it, and what does it cost? | a word from a description; the name is shared | P-1747 Submersible pump 0.75kW, by S-194 Inn Pumpen, 189.00 (P-1233 and P-2893 share its name) |
| 6 | Which customers have bought the valve we sell for breweries and dairies, and how many units has each bought? Leave out cancelled orders. | search, then a walk and a sum | P-1068 Butterfly valve DN50: Echo Trading 74, Finch Engineering 40, Orchard Tide Logistics 35, Flint Supplies 17, Bay Trading 15, Crystal Hardware 11, Tiger Utilities 5, Bridge Bay Motors 3 |
| 7 | One of our makers is a family firm on Lake Como that also repairs what it sells. Which of our customers in Germany have ordered its goods? Count every order, whatever its status. | search in a supplier's notes, then a walk of three hops | S-235 Bellagio Valvole: C-572, C-578, C-654, C-1700, C-1894, C-1979, C-2519 |
| 8 | The maker that ships by rail from Gdansk: how many units of its goods are on orders that are still open? | search, then an aggregate | S-171 Baltic Okucia: 1001 |
| 9 | A customer asks for the product of ours that best matches the words "seawater pump". Which one is it? | the most relevant, which BM25F picks and a count of words held would not | P-2639 Seawater lift pump 1.5kW (15.6637, against P-1842 Multistage pump 4kW, 7.9077); by distinct words held, ties by key, and by words held, repeats counted, P-1842 comes first |
| 10 | What has Quillon Aerospace ordered from us this year? | the answer that does not exist | no customer is called so or anything near it; 191 customers' names hold Aerospace |

Task 9's P-1842 is a multistage pump whose description repeats
"pump" and names seawater once; P-2639's name and description both
hold "seawater", the rarer word. BM25F weighs a word by its rarity and
saturates its repeats, so P-2639 ranks first. A count of the query's
words held, repeats counted, puts P-1842 first (8 against 4); counted
once each, the two tie at 2 and P-1842 leads only on its key.

The lake, the tasks, their reference answers and the runner were
committed at 66ff9f08 (18:10:33 UTC), before any run. The shared name
was renamed at b654127f (19:37:21 UTC), before runs 1 to 4, with every
key and answer unchanged (§ Recording the walk).

## How a run is counted

As walk 7 counts it. Each task is counted from the run's own calls,
numbered as `run-<R>.txt` numbers them.

- **First try:** the first call the agent made for the task answered
  it, without a refusal, and the agent's final answer is the reference
  answer.
- **Right:** the agent's final answer is the reference answer.
- **Calls:** every call the agent made for the task, its checks
  included, wherever in the run it was made. A call that asked for two
  tasks' records at once counts for the first.
- **Refusals:** each refused call, and whether it was understood.
- **Tool:** the tool whose answer named the record the task is about,
  then the one the final answer was read from.

Every run began with `describe` and `pin`, counted apart as set-up.

## The runs

Calls are numbered as `run-<R>.txt` numbers them. *Found → read*
names the tool that found the record the task is about, then the one
the answer was read from.

| Task | Found → read | R1 first try | R1 calls | R2 first try | R2 calls | R3 first try | R3 calls | R4 first try | R4 calls |
|---|---|---|---|---|---|---|---|---|---|
| set-up | | | 2 (1–2) | | 2 (1–2) | | 2 (1–2) | | 2 (1–2) |
| 1 | R1, R2: search, then a find; R3, R4: a find's `contains` → find | no, 3 refused | 7 (3, 5, 9, 11, 13–15) | no, 3 refused | 6 (3–5, 7, 10–11) | yes | 4 (3–4, 6–7) | yes | 4 (3, 5–6, 8) |
| 2 | R1, R2: search; R3, R4: `contains` → find | yes | 3 (4, 8, 16) | yes | 3 (12–14) | yes | 2 (8–9) | yes | 2 (9–10) |
| 3 | the task 1 call that found the three → cypher | yes | 3 (6, 17–18) | yes | 2 (15–16) | yes | 1 (10) | yes | 2 (11, 13) |
| 4 | find | **wrong** | 1 (19) | **wrong** | 1 (17) | **wrong** | 1 (11) | **wrong** | 1 (12) |
| 5 | search → get | yes | 2 (20–21) | yes | 2 (18–19) | yes | 2 (12–13) | yes | 2 (14–15) |
| 6 | search → cypher | yes | 2 (22–23) | yes | 2 (20, 23) | yes | 2 (14–15) | yes | 2 (16–17) |
| 7 | search → cypher | yes | 2 (24, 26) | yes | 3 (21, 24, 26) | yes | 2 (16–17) | yes | 3 (18, 20, 22) |
| 8 | search → cypher | yes | 3 (25, 27–28) | yes | 2 (22, 25) | yes | 3 (18–20) | yes | 2 (19, 21) |
| 9 | search | yes | 1 (29) | no, 27 refused | 2 (27–28) | yes | 1 (21) | yes | 1 (23) |
| 10 | search and find: none | yes | 4 (7, 10, 12, 31) | yes, 8 refused | 3 (6, 8–9) | yes | 1 (5) | yes | 2 (4, 7) |
| a check | `sys_aging_rule_find` | | 1 (30) | | | | | | |

Every run gave the reference answer to nine tasks of ten, and the same
nine. Task 4 was answered in every run for the Portuguese Halden
Supplies alone (O-11833, 2026-09-15, open), task 3's customer, though
every run had found the three in its first calls and named them in
task 3's answer. Run 1's task 3 answer says "four customers
called Halden Supplies" and names three countries; its units are the
reference answer. No run invented a customer for task 10, and each
said why it could not be answered. No run cleared its `pin`.

The characters the agent wrote to the tools and those they answered,
by task:

| Task | R1 wrote, answered | R2 | R3 | R4 |
|---|---|---|---|---|
| set-up | 4, 9,466 | 4, 9,466 | 4, 9,466 | 4, 9,466 |
| 1 | 668, 11,428 | 575, 6,972 | 492, 3,236 | 504, 3,341 |
| 2 | 142, 24,555 | 212, 3,436 | 142, 6,774 | 125, 6,762 |
| 3 | 383, 1,543 | 331, 621 | 241, 420 | 231, 556 |
| 4 | 115, 746 | 113, 736 | 115, 746 | 113, 736 |
| 5 | 121, 5,044 | 121, 5,044 | 103, 2,019 | 104, 3,475 |
| 6 | 337, 2,727 | 342, 2,727 | 353, 2,727 | 334, 2,727 |
| 7 | 363, 1,186 | 440, 1,667 | 339, 1,186 | 421, 1,667 |
| 8 | 525, 2,759 | 316, 1,588 | 396, 1,789 | 263, 1,733 |
| 9 | 88, 2,903 | 175, 3,525 | 89, 5,024 | 88, 2,903 |
| 10 | 266, 11,504 | 297, 6,749 | 77, 2,116 | 147, 2,193 |

`walk-8/calls.py` gives them, from each run and the calls above:
`python3 calls.py run-1.jsonl set-up=1-2 1=3,5,9,11,13-15 …`. Run 1's
task 2 holds a first page of 100 suppliers (21,391 characters), read
before it searched.

The refusals:

- **R1 3, R2 3:** the first call of each run, a find whose `where`
  wrote `name contains 'Kestr'` (R2) or `name contains 'Kestral'`
  (R1), with an infix `contains`: "the expression `…` does not parse:
  unexpected input after the end of the expression (at character 5)".
  Both read it as "the grammar lacks `contains`" and turned to
  `search`, which found nothing for the misspelled word. R1 found
  Kestrel by searching "Kestrel" (11), a guess at the spelling; R2 by
  `starts_with(name, 'Kest')` (7), four calls on. Not understood: the
  refusal says where the parse stopped, not that the grammar matches a
  text by a function, and the `where` description did not say so
  (finding 1).
- **R2 8:** `ends_with(name, 'Aerospace')`: "there is no function
  `ends_with` (at character 26)". The next call left it out.
  Understood.
- **R2 27:** a hybrid search of "seawater pump" without `field`:
  "what is given does not fit the tool's `inputSchema`: not of the
  expected shape". The agent concluded that "hybrid apparently needs a
  vector field I don't have" and searched lexically, which answered.
  Understood by the rule above, but from what it guessed, not from the
  refusal, which names no member (finding 2, lakelands#1471).

## The sums

| | R1 | R2 | R3 | R4 |
|---|---|---|---|---|
| right on the first try, of 10 | 8 | 7 | 9 | 9 |
| right at all, of 10 | 9 | 9 | 9 | 9 |
| wrong, of them invented | 1 (task 4), 0 | 1 (task 4), 0 | 1 (task 4), 0 | 1 (task 4), 0 |
| calls, of them on the tasks | 31, 28 | 28, 26 | 21, 19 | 23, 21 |
| refusals, of them understood | 1, 0 | 3, 2 | 0 | 0 |
| `search` calls, refused; characters written, answered | 12, 0; 865, 32,097 | 10, 1; 842, 21,611 | 7, 0; 579, 14,703 | 7, 0; 565, 14,041 |
| finds (`<type>_find`, `sys_*_find`), refused; characters | 11, 1; 902, 29,343 | 11, 2; 1,059, 9,155 | 7, 0; 761, 8,878 | 8, 0; 825, 9,475 |
| `cypher` calls, refused; characters | 5, 0; 1,227, 2,821 | 4, 0; 1,005, 2,110 | 4, 0; 991, 2,267 | 4, 0; 907, 2,148 |
| characters written to every tool, answered by them | 3,014, 73,916 | 2,926, 42,531 | 2,351, 35,503 | 2,334, 35,559 |

Runs 3 and 4 made fewer calls than runs 1 and 2, 21 and 23 against 31
and 28, and were answered fewer characters: the difference is tasks 1,
2 and 10, the three
misspelled or missing names, which took 14 and 12 calls in runs 1 and
2 and 7 and 8 in runs 3 and 4.

## What the runs show

- **Search, then a walk, works as an agent writes it.** Tasks 5 to 8
  name a record by a word of its description or notes. In all four
  runs the first `search` of each put that record first, and the next
  call walked from its key through `cypher` (6, 7, 8) or read it with
  `<type>_get` (5). None of these 16 searches was refused or
  followed by a second one, and every answer was the reference
  answer. The walks were written by key in every run, `{id: $s}` or
  `{supplier: $sid}`, never by the name the search had found.
- **BM25F picked task 9's product, and the agents trusted the
  order.** Every run asked "seawater pump" once lexically and answered
  with the first hit, P-2639, above P-1842 (15.7
  against 7.9, in all four runs); a count of the words held, repeats
  counted, would have put P-1842 first.
  Run 2 first tried a hybrid search, refused (finding 2).
- **A misspelled name is where the door told the agent least.** The
  lexical search matches a word as written, so "Kestral" and
  "Guarnizoni" match nothing, and no tool said so. Runs 1 and 2 found
  Kestrel by a guess at the spelling and by `starts_with`, after a
  refused infix `contains` (finding 1); task 1 took them 7 and 6 calls.
  Runs 3 and 4, on the fixed `where`, wrote `contains(name, …)` from
  their first call, found Kestrel and Torino Guarnizioni at once, and
  took 4 calls. Both still searched the misspelled names, run 3
  expecting the search to "tolerate spelling variants" (finding 4),
  and run 4, after the text that says it does not, "to check for case
  or spelling variants". The search found nothing, and neither run was
  misled by it.
- **A shared name was told apart where the task told it apart, and
  not where it did not.** Task 3's "the customer in Portugal" was
  answered for C-2113 in every run. Task 4, which names Halden
  Supplies with nothing to tell the three apart, was answered in every
  run for the same C-2113, carried over from task 3. No run said that
  task 4's name was shared. This is the prompt's order as much as the
  door: put after task 3, task 4 reads as its follow-up. The door
  answered every call about it rightly, and nothing a tool says could
  have changed which customer the agent took the question to mean.
- **The answer that does not exist was not invented.** Task 10 was
  searched, filtered by prefix and looked for among retired records in
  every run, and every run said that no such customer is in the lake
  and why; three of the four offered to look again given an id or
  another spelling.
- **Every run checked.** Each checked the order statuses or the
  countries before trusting a filter on them, and run 1 summed task 8
  a second way. Every check agreed.

## Found on the walk

Each finding below is a text, fixed in this walk's change, with the
text as it was, what the agent did, and the text as it is now. Two
regression tests (`crates/lakelands-api/src/tools/tests.rs`,
`a_where_and_a_search_mode_say_what_their_refusals_ask` and
`a_search_text_says_a_word_matches_only_as_written`) hold the four.
One finding has a code half, filed as lakelands#1471 in 0.21.0. It is
not a defect of what 0.20.0 ships: the refusal predates BM25F, and the
texts now say what a hybrid search needs.

| Runs and calls | Finding | Issue |
|---|---|---|
| R1 3, R2 3 | A find's `where` matches a text by a function, `contains(f, 'x')`, and the agent writes it infix, as `cypher` does; the refusal says where the parse stopped | |
| R2 27 | A hybrid search needs `field`, nothing in the schema says so, and the refusal names no member | the code half is lakelands#1471 |
| R2 27 | `vector` is optional for a hybrid search, and nothing says that either | |
| R3 4–5, R1 5, R2 4–6 | A search's word matches only as written; the agent expected it to tolerate a misspelling | |

**A find's `where`** (every `<type>_find`, and every `sys_*_find`):

- Before: "A condition in the expression grammar, e.g. status = 'open'
  and amount > 100; a list is written in parentheses, status in
  ('open', 'paid'), not in brackets as a `cypher` statement writes it."
- After: the same, then "; a text is matched by a function,
  contains(name, 'Kest') or starts_with(name, 'Kest'), not by a word
  between its operands."

**A search's `mode`, `field` and `vector`** (`search`):

- Before: `mode` the enum alone; `field` and `vector` no description.
- After, `mode`: "lexical ranks `text` in the searchable fields;
  vector ranks `vector` against `field`; hybrid fuses both and needs
  `text` and `field`, its `vector` optional."
- `field`: "vector and hybrid, which both need it: the vector field
  searched, one the type declares."
- `vector`: "vector, and hybrid where the caller has one: the query
  vector."

**A search's `text`** (`search`), found in run 3 and fixed for run 4:

- Before: no description.
- After: "lexical and hybrid: the words ranked; a word matches a
  field's word as written, its case and accents aside and stemmed
  where the type declares a `language`, so a misspelled word matches
  nothing: for a name half known, a find's `where` with starts_with or
  contains."

Runs 3 and 4 wrote every `where` with `contains` or `starts_with`
from their first call, and neither tried a hybrid search; no call of
either was refused. Run 4 still searched the misspelled names (above),
so whether the `text` description changes what an agent does is not
shown; it made the right call first and searched to check.

The refusals themselves are unchanged. The `where` refusal is right:
it says where the parse stopped, and with the description beside it,
what to write instead. The hybrid refusal is the code half of
finding 2, lakelands#1471.

The server's MCP transcripts
(`crates/lakelands-server/tests/transcripts/`) were re-recorded for
the new texts.

The door checks (`crates/lakelands-door-checks`, run over MCP by
`crates/lakelands-client/tests/door_checks.rs`) now hold this walk's
path: a record found by `search` on a word of its name, then walked
from by its key through a `cypher` statement
(`a_record_found_by_search_is_walked_from_by_its_key_through_cypher`),
beside walk 7's record found through `describe` and `cypher`.

## What was not measured

- **Whether the fixed texts help another agent.** Runs 3 and 4 are
  one run each on the fixed doors, by the same agent as runs 1 and 2.
  Their first tries show that the `where` and `mode` texts were read
  and enough for this agent on these tasks, not more; run 4 shows the
  `text` description did not stop a check by search.
- **A shared name with nothing to tell it apart, on its own.** Task 4
  followed task 3's customer in every run; a task that names a shared
  name first, with no task before it, is not measured.
- **Hybrid and vector search.** The lake declares no vector field, so
  no run could rank by one; BM25F was measured through `lexical` only.
- **Stemming.** The types declare no `language`, so no word was
  stemmed.
- **Four runs is a small sample.** Runs 1 and 2 differ on task 9 (R2
  tried hybrid first) and on task 10; either could fall the other way.

## Recording the walk

The runs were made on 2026-10-26, each on a fresh lake from
`walk-8/setup.sh`: runs 1 and 2 against develop at e74216d4, at the
same time, from 19:37 to 19:40 UTC; run 3 against cb9a1081, from 19:41
to 19:43 UTC; run 4 against 00f36cec, from 19:49 to 19:52 UTC. The
agent's answers were compared with the reference answers above by
hand, field by field.

Two runs and a third were made before them, from 19:20 UTC, on a lake
whose shared customer name was a word the vocabulary gate refuses
anywhere in the repository. The withdrawn runs 1 and 2 were
committed at 9e8ecd6b, removed at b654127f and replaced at d24c3d70; their transcripts,
and the generator, prompt and `answers.py` of 66ff9f08, hold that word
in the branch's history, kept so that the pre-registration commit
stands as it was made. The withdrawn run 3 was never committed. The
name was changed at b654127f in the generator, the prompt and
`answers.py`, every key, record and reference answer unchanged, and
the runs were made again. The withdrawn runs 1 and 2 met the same
refusals (the infix `contains` in both, the hybrid without `field` in
one) and the same task 4, and were right on the other nine tasks; the
withdrawn run 3 was not read.
