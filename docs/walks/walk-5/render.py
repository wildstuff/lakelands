#!/usr/bin/env python3
"""Renders one run of walk 5 (its `run-N.jsonl`: the agent's and the
tools' messages, as `claude -p --output-format stream-json` wrote them) as
plain text: what the agent thought and wrote, each call and each answer."""
import json
import sys

n = 0
for line in open(sys.argv[1]):
    m = json.loads(line)
    for c in m["message"]["content"]:
        if not isinstance(c, dict):
            continue
        if c["type"] == "thinking" and c.get("thinking", "").strip():
            print("THINKING:", c["thinking"].strip(), "\n")
        elif c["type"] == "text":
            print("AGENT:", c["text"].strip(), "\n")
        elif c["type"] == "tool_use":
            n += 1
            name = c["name"].removeprefix("mcp__lake__")
            print(f"CALL {n}: {name} {json.dumps(c['input'], ensure_ascii=False)}\n")
        elif c["type"] == "tool_result":
            body = c["content"]
            if isinstance(body, list):
                body = "\n".join(x.get("text", json.dumps(x)) for x in body)
            print("ANSWER" + (" (isError)" if c.get("is_error") else "") + ":\n" + body.rstrip() + "\n")
