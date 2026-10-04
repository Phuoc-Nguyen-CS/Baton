#!/usr/bin/env python3
"""M0.6: compare usage reported by OTLP metrics, OTLP api_request events, the status line,
and the transcript, per session. Usage: python3 compare.py <out_dir>"""
import json
import os
import sys
from collections import defaultdict

out = sys.argv[1]


def lines(name):
    path = os.path.join(out, name)
    return [json.loads(l) for l in open(path)] if os.path.exists(path) else []


def attrs(kvs):
    res = {}
    for kv in kvs or []:
        v = kv.get("value", {})
        res[kv["key"]] = next(iter(v.values()), None) if v else None
    return res


TOKEN_TYPES = ("input", "output", "cacheRead", "cacheCreation")
metric_tokens = defaultdict(lambda: defaultdict(float))
metric_cost = defaultdict(float)
event_tokens = defaultdict(lambda: defaultdict(float))
event_count = defaultdict(int)
exports = defaultdict(list)
metric_names, event_names = set(), set()
pii_keys = set()

for rec in lines("otlp.jsonl"):
    body, path = rec["body"], rec["path"]
    exports[path].append(rec["ms"])
    for rm in body.get("resourceMetrics", []):
        for sm in rm.get("scopeMetrics", []):
            for m in sm.get("metrics", []):
                metric_names.add(m["name"])
                for dp in (m.get("sum") or m.get("gauge") or {}).get("dataPoints", []):
                    a = attrs(dp.get("attributes"))
                    sid = a.get("session.id", "?")
                    val = dp.get("asDouble", dp.get("asInt", 0))
                    if m["name"] == "claude_code.token.usage":
                        metric_tokens[sid][a.get("type")] += float(val)
                    elif m["name"] == "claude_code.cost.usage":
                        metric_cost[sid] += float(val)
    for rl in body.get("resourceLogs", []):
        for sl in rl.get("scopeLogs", []):
            for lr in sl.get("logRecords", []):
                a = attrs(lr.get("attributes"))
                pii_keys.update(k for k in a if k.startswith("user.") or k == "organization.id")
                name = a.get("event.name")
                event_names.add(name)
                if name == "api_request":
                    sid = a.get("session.id", "?")
                    event_count[sid] += 1
                    for k, t in (("input_tokens", "input"), ("output_tokens", "output"),
                                 ("cache_read_tokens", "cacheRead"), ("cache_creation_tokens", "cacheCreation")):
                        event_tokens[sid][t] += float(a.get(k) or 0)
                    event_tokens[sid]["cost_usd"] += float(a.get("cost_usd") or 0)

print("== OTLP exports by path:", {p: len(v) for p, v in exports.items()})
print("   metric names:", sorted(metric_names))
print("   event names:", sorted(n for n in event_names if n))
print("   identity attributes on events:", sorted(pii_keys))

status = lines("statusline.jsonl")
print(f"== status line invocations: {len(status)}")
transcripts = {}
by_sid = defaultdict(list)
for s in status:
    i = s["input"]
    by_sid[i.get("session_id", "?")].append(s)
    if i.get("transcript_path"):
        transcripts[i.get("session_id")] = i["transcript_path"]
for sid, ss in by_sid.items():
    last = ss[-1]["input"]
    print(f"   {sid[:8]}: {len(ss)} calls; last cost.total_cost_usd={last.get('cost', {}).get('total_cost_usd')} "
          f"context_window.used_percentage={last.get('context_window', {}).get('used_percentage')} "
          f"rate_limits={json.dumps(last.get('rate_limits'))}")
for h in lines("hooks.jsonl"):
    i = h["input"] if isinstance(h["input"], dict) else {}
    if i.get("transcript_path"):
        transcripts.setdefault(i.get("session_id"), i["transcript_path"])

print("== tokens by source (per session)")
for sid in sorted(set(metric_tokens) | set(event_tokens) | set(transcripts)):
    print(f"   session {sid[:8]}")
    print(f"     OTLP metrics : " + " ".join(f"{t}={int(metric_tokens[sid][t])}" for t in TOKEN_TYPES)
          + f" cost_usd={metric_cost[sid]:.5f}")
    print(f"     OTLP events  : {event_count[sid]} api_request; " + " ".join(f"{t}={int(event_tokens[sid][t])}" for t in TOKEN_TYPES)
          + f" cost_usd={event_tokens[sid]['cost_usd']:.5f}")
    path = transcripts.get(sid)
    if path and os.path.exists(path):
        per_msg = {}
        entries = 0
        for line in open(path):
            d = json.loads(line)
            m = d.get("message")
            if d.get("type") == "assistant" and isinstance(m, dict) and m.get("usage"):
                entries += 1
                per_msg[m.get("id")] = m["usage"]  # one message can span several entries; keep the last
        tot = defaultdict(int)
        for u in per_msg.values():
            tot["input"] += u.get("input_tokens", 0)
            tot["output"] += u.get("output_tokens", 0)
            tot["cacheRead"] += u.get("cache_read_input_tokens", 0)
            tot["cacheCreation"] += u.get("cache_creation_input_tokens", 0)
        print(f"     transcript   : {len(per_msg)} messages from {entries} entries; " + " ".join(f"{t}={tot[t]}" for t in TOKEN_TYPES))
