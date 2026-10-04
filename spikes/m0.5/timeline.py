#!/usr/bin/env python3
"""M0.5: merge enqueue/delivery/marks, hook events, ack files and the worker transcript's
user-side entries from OUT (argv[1]) into one timeline."""
import json
import os
import sys
from datetime import datetime

out = sys.argv[1]
items = []


def lines(path):
    return [json.loads(l) for l in open(path)] if os.path.exists(path) else []


for r in lines(os.path.join(out, "steer.jsonl")):
    kind = next(k for k in ("enqueued", "delivered", "mark") if k in r)
    items.append((r["ms"], kind, f"{r[kind]} {r.get('via', r.get('queue', ''))}"))
for r in lines(os.path.join(out, "acks.jsonl")):
    items.append((r["ms"], "ack file", r["ack"]))
transcripts = set()
for r in lines(os.path.join(out, "hooks.jsonl")):
    i = r["input"] if isinstance(r["input"], dict) else {}
    if i.get("transcript_path"):
        transcripts.add(i["transcript_path"])
    e = i.get("hook_event_name")
    info = {"PreToolUse": (i.get("tool_input") or {}).get("command") or i.get("tool_name"),
            "PostToolUse": (i.get("tool_input") or {}).get("command") or i.get("tool_name"),
            "UserPromptSubmit": repr(i.get("prompt", ""))[:120],
            "Stop": f"active={i.get('stop_hook_active')} " + repr(i.get("last_assistant_message", ""))[:120],
            "SessionStart": i.get("source"), "Notification": i.get("notification_type"),
            "SessionEnd": i.get("reason")}.get(e, "")
    items.append((r["ms"], f"hook {e} [{i.get('session_id', '?')[:8]}]", info))
# user-side transcript entries (prompts, peer messages, hook context) with their timestamps
for t in transcripts:
    if not os.path.exists(t):
        continue
    for line in open(t):
        d = json.loads(line)
        if d.get("type") not in ("user", "attachment") or not d.get("timestamp"):
            continue
        ms = int(datetime.fromisoformat(d["timestamp"].replace("Z", "+00:00")).timestamp() * 1000)
        if d["type"] == "attachment":
            a = d.get("attachment", {})
            text = json.dumps(a)
            if "Baton" in text or "Message from" in text or a.get("type") in ("queued_command", "peer_message"):
                items.append((ms, f"transcript attach:{a.get('type')}", text[:150]))
        else:
            c = d.get("message", {}).get("content")
            if isinstance(c, str):
                items.append((ms, "transcript user", c[:150].replace("\n", " ")))

items.sort()
t0 = items[0][0] if items else 0
for t, kind, info in items:
    print(f"+{(t - t0) / 1000:7.1f}s  {kind:34} {info}")
