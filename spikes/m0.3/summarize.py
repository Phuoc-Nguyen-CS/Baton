#!/usr/bin/env python3
"""Summarize M0.3 evidence: hooks.jsonl (argv[1]) grouped by label and session, and mcp.log (argv[2])."""
import json
import sys
from collections import defaultdict


def detail(event, inp):
    if event == "SessionStart":
        return f"source={inp.get('source')} model={inp.get('model', '')}"
    if event == "InstructionsLoaded":
        return f"{inp.get('file_path')} ({inp.get('load_reason')}, {inp.get('memory_type', '')})"
    if event in ("PreToolUse", "PermissionRequest", "PermissionDenied"):
        cmd = (inp.get("tool_input") or {}).get("command", "")
        return f"{inp.get('tool_name')} {cmd}".strip()
    if event == "Stop":
        return f"stop_hook_active={inp.get('stop_hook_active')} last={inp.get('last_assistant_message', '')[:160]!r}"
    if event == "Notification":
        return f"{inp.get('notification_type', '')} {inp.get('message', '')[:100]!r}"
    if event == "SessionEnd":
        return f"reason={inp.get('reason')}"
    return ""


groups = defaultdict(list)
sockets = defaultdict(set)
for line in open(sys.argv[1]):
    rec = json.loads(line)
    inp = rec["input"] if isinstance(rec["input"], dict) else {}
    key = (rec["label"], inp.get("session_id", "?"))
    groups[key].append((rec["ms"], inp.get("hook_event_name", "?"), detail(inp.get("hook_event_name"), inp)))
    sockets[key].add(bool(rec["socket"]))

for (label, sid), events in groups.items():
    print(f"== {label} session {sid}  socket_env={sorted(sockets[(label, sid)])}")
    t0 = events[0][0]
    for t, event, info in events:
        print(f"  +{(t - t0) / 1000:6.1f}s {event:18} {info}")

print("== mcp.log")
try:
    for line in open(sys.argv[2]):
        rec = json.loads(line)
        print(f"  {rec['ms']} {rec['method']}")
except FileNotFoundError:
    print("  (no mcp.log: server never started)")
