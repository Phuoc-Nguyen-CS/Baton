#!/usr/bin/env python3
"""M0.5 hook: deliver Baton's queued instructions into a worker's context.

Usage in settings: python3 inject.py <queue_dir> <log>  (for PostToolUse and Stop)
Pops every file in <queue_dir> (oldest first), returns them as additionalContext for the
event that fired, and logs each delivery to <log>. Prints nothing when the queue is empty.
"""
import json
import os
import sys
import time

queue, log = sys.argv[1], sys.argv[2]
event = json.load(sys.stdin)
name = event.get("hook_event_name")
os.makedirs(queue, exist_ok=True)
items = sorted(f for f in os.listdir(queue) if f.endswith(".json"))
if not items:
    sys.exit(0)
texts = []
with open(log, "a") as f:
    for item in items:
        path = os.path.join(queue, item)
        msg = json.load(open(path))
        os.remove(path)
        texts.append(f"Message from Baton ({msg['id']}): {msg['text']}")
        f.write(json.dumps({"ms": int(time.time() * 1000), "delivered": msg["id"], "via": name,
                            "session_id": event.get("session_id")}) + "\n")
print(json.dumps({"hookSpecificOutput": {"hookEventName": name, "additionalContext": "\n".join(texts)}}))
