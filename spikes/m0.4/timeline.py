#!/usr/bin/env python3
"""M0.4: merge hook events, inbox requests/answers, responder actions, blocked pushes and
agents --json rows from OUT (argv[1]) into one timeline, then list the transcript's tool results."""
import glob
import json
import os
import sys

out = sys.argv[1]
items = []


def lines(path):
    return [json.loads(l) for l in open(path)] if os.path.exists(path) else []


transcript = None
for r in lines(os.path.join(out, "hooks.jsonl")):
    i = r["input"] if isinstance(r["input"], dict) else {}
    transcript = transcript or i.get("transcript_path")
    e = i.get("hook_event_name")
    cmd = (i.get("tool_input") or {}).get("command", "")
    info = {"PreToolUse": cmd, "PermissionRequest": cmd, "PostToolUse": cmd,
            "PostToolUseFailure": f"{cmd} :: {str(i.get('error', ''))[:80]}",
            "Notification": i.get("notification_type", ""),
            "Stop": repr(i.get("last_assistant_message", ""))[:300],
            "SessionEnd": i.get("reason", "")}.get(e, "")
    items.append((r["ms"], f"hook {e}", info))
for path in glob.glob(os.path.join(out, "inbox", "requests", "*.json")):
    q = json.load(open(path))
    items.append((q["ms"], "inbox request", f"{q['id']} {(q.get('tool_input') or {}).get('command', '')}"))
for r in lines(os.path.join(out, "inbox", "responder.log")):
    items.append((r["ms"], "responder", f"{r['id']} " + ("answered " + r["answered"] if "answered" in r else f"seen, plan={r['plan']}")))
for r in lines(os.path.join(out, "push-blocked.jsonl")):
    items.append((r["ms"], "push blocked", r["command"]))
for r in lines(os.path.join(out, "rows.jsonl")):
    items.append((r["ms"], "agents row", r["row"]))

items.sort()
t0 = items[0][0] if items else 0
for t, kind, info in items:
    print(f"+{(t - t0) / 1000:6.1f}s  {kind:20} {info}")

if transcript and os.path.exists(transcript):
    print("== tool results in transcript")
    for line in open(transcript):
        d = json.loads(line)
        m = d.get("message")
        if isinstance(m, dict) and isinstance(m.get("content"), list):
            for c in m["content"]:
                if c.get("type") == "tool_result":
                    txt = c.get("content")
                    txt = txt if isinstance(txt, str) else json.dumps(txt)
                    print(f"  is_error={c.get('is_error')}: {txt[:200]!r}")
