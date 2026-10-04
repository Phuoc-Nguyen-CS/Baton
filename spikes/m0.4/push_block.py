#!/usr/bin/env python3
"""M0.4 PreToolUse hook: deny any Bash command that pushes, and record the block in argv[1].

This is the visible version of a `Bash(git push:*)` deny rule: Baton sees every attempt.
"""
import json
import re
import sys
import time

log = sys.argv[1]
event = json.load(sys.stdin)
command = (event.get("tool_input") or {}).get("command", "")
if event.get("tool_name") == "Bash" and re.search(r"\bgit\s+push\b|\bgh\s+pr\b", command):
    with open(log, "a") as f:
        f.write(json.dumps({"ms": int(time.time() * 1000), "session_id": event.get("session_id"),
                            "command": command}) + "\n")
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": "Baton policy: pushing needs the owner's approval. Leave changes committed locally.",
    }}))
