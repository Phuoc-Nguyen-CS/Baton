#!/usr/bin/env python3
"""M0.4 PermissionRequest hook: ask a Baton-style inbox, wait a bounded time for the answer.

Writes <inbox>/requests/<id>.json, then polls <inbox>/answers/<id>.json for up to
argv[2] seconds. Prints an allow/deny decision when answered; prints nothing when
not, so Claude Code falls back to its own prompt.
Usage: python3 perm_hook.py <inbox> <wait_seconds>
"""
import json
import os
import sys
import time
import uuid

inbox, wait = sys.argv[1], float(sys.argv[2])
event = json.load(sys.stdin)
rid = uuid.uuid4().hex[:12]
request = {
    "id": rid,
    "ms": int(time.time() * 1000),
    "session_id": event.get("session_id"),
    "tool_name": event.get("tool_name"),
    "tool_input": event.get("tool_input"),
    "permission_mode": event.get("permission_mode"),
}
os.makedirs(os.path.join(inbox, "requests"), exist_ok=True)
tmp = os.path.join(inbox, "requests", f".{rid}.tmp")
with open(tmp, "w") as f:
    json.dump(request, f)
os.rename(tmp, os.path.join(inbox, "requests", f"{rid}.json"))

answer_path = os.path.join(inbox, "answers", f"{rid}.json")
deadline = time.time() + wait
while time.time() < deadline:
    if os.path.exists(answer_path):
        answer = json.load(open(answer_path))
        decision = {"behavior": answer["behavior"]}
        if answer["behavior"] == "deny":
            decision["message"] = answer.get("message", "Denied by Baton.")
        print(json.dumps({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}}))
        sys.exit(0)
    time.sleep(0.2)
# no answer in time: no output, so the native prompt appears
