#!/usr/bin/env python3
"""M0.3 logging hook: append {label, ms, socket, input} as one JSON line to the log in argv[1].

Prints nothing and exits 0, so it never makes a decision for Claude Code.
Usage in settings: "command": "python3 /abs/hook.py /abs/hooks.jsonl <label>"
"""
import json
import os
import sys
import time

log, label = sys.argv[1], sys.argv[2]
raw = sys.stdin.read()
try:
    data = json.loads(raw)
except ValueError:
    data = raw
record = {
    "label": label,
    "ms": int(time.time() * 1000),
    "socket": os.environ.get("CLAUDE_CODE_MESSAGING_SOCKET", ""),
    "input": data,
}
with open(log, "a") as f:
    f.write(json.dumps(record) + "\n")
