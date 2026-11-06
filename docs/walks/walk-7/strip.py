#!/usr/bin/env python3
"""Strips one run of `claude -p --output-format stream-json` (argv 1) as
walks 5 and 6 strip theirs, onto stdout: the agent's and the tools'
messages alone; no harness lines, no session, message or request ids, no
thinking signatures and no empty thinking, no usage; the walk's scratch
directory (argv 2) reads `<walk>`."""
import json
import sys

src, scratch = sys.argv[1], sys.argv[2]
for line in open(src):
    m = json.loads(line)
    if m.get("type") not in ("assistant", "user"):
        continue
    msg = m["message"]
    content = []
    for c in msg.get("content", []):
        if isinstance(c, dict) and c.get("type") == "thinking":
            if not c.get("thinking", "").strip():
                continue
            c = {"type": "thinking", "thinking": c["thinking"]}
        content.append(c)
    if not content:
        continue
    out = {"type": m["type"], "timestamp": m.get("timestamp")}
    keep = {"role": msg["role"], "content": content}
    if "model" in msg:
        keep = {"role": msg["role"], "model": msg["model"], "content": content}
    out["message"] = keep
    print(json.dumps(out, ensure_ascii=False).replace(scratch, "<walk>"))
