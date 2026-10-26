#!/usr/bin/env python3
"""Lists every call of one run (its `run-R.jsonl`, argv 1), numbered as
`docs/walks/walk-5/render.py` numbers them: the tool, the characters the
agent wrote to it, the characters it answered, and whether it refused.
With each task's calls after the run, as numbers and ranges
(`1=3,5,9-11 2=4,8 ...`), sums the calls and characters of each task
instead."""
import json
import sys

calls, results = [], {}
for line in open(sys.argv[1]):
    for c in json.loads(line)["message"]["content"]:
        if not isinstance(c, dict):
            continue
        if c["type"] == "tool_use":
            calls.append((c["id"], c["name"].removeprefix("mcp__lake__"),
                          len(json.dumps(c["input"], ensure_ascii=False))))
        elif c["type"] == "tool_result":
            body = c["content"]
            if isinstance(body, list):
                body = "\n".join(x.get("text", json.dumps(x)) for x in body)
            results[c["tool_use_id"]] = (len(body), bool(c.get("is_error")))

rows = [(n, tool, wrote, *results.get(i, (0, False))) for n, (i, tool, wrote) in enumerate(calls, 1)]
if len(sys.argv) == 2:
    for n, tool, wrote, answered, refused in rows:
        print(f"{n}\t{tool}\t{wrote}\t{answered}" + ("\trefused" if refused else ""))
else:
    print("task\tcalls\twrote\tanswered\trefused")
    for arg in sys.argv[2:]:
        task, spans = arg.split("=")
        held = set()
        for span in spans.split(","):
            a, _, b = span.partition("-")
            held.update(range(int(a), int(b or a) + 1))
        mine = [r for r in rows if r[0] in held]
        print(f"{task}\t{len(mine)}\t{sum(r[2] for r in mine)}\t{sum(r[3] for r in mine)}\t{sum(r[4] for r in mine)}")
