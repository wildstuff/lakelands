#!/usr/bin/env python3
"""Prints the tool list the MCP door shows: argv is the command that
serves it on stdio (`lakelands --root R --as analyst serve --mcp`)."""
import json
import subprocess
import sys

p = subprocess.Popen(sys.argv[1:], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def ask(i, method, params):
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n")
    p.stdin.flush()
    while True:
        m = json.loads(p.stdout.readline())
        if m.get("id") == i:
            return m["result"]
ask(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "walk-7", "version": "0"}})
p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
tools = ask(2, "tools/list", {})["tools"]
p.stdin.close()
p.wait()
print(json.dumps(tools, indent=2, ensure_ascii=False))
