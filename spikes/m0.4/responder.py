#!/usr/bin/env python3
"""M0.4 stand-in for Baton: answer permission requests in <inbox> by a fixed policy.

A command containing ALLOW is allowed and one containing DENY is denied, each after
<delay> seconds; anything else gets no answer. Runs for <seconds>, logging to <inbox>/responder.log.
Usage: python3 responder.py <inbox> <delay> <seconds>
"""
import json
import os
import sys
import time

inbox, delay, run_for = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
req_dir, ans_dir = os.path.join(inbox, "requests"), os.path.join(inbox, "answers")
os.makedirs(req_dir, exist_ok=True)
os.makedirs(ans_dir, exist_ok=True)
log = open(os.path.join(inbox, "responder.log"), "a", buffering=1)
seen = set()
end = time.time() + run_for
while time.time() < end:
    for name in sorted(os.listdir(req_dir)):
        if not name.endswith(".json") or name in seen:
            continue
        seen.add(name)
        req = json.load(open(os.path.join(req_dir, name)))
        text = json.dumps(req.get("tool_input"))
        behavior = "allow" if "ALLOW" in text else "deny" if "DENY" in text else None
        log.write(json.dumps({"ms": int(time.time() * 1000), "id": req["id"], "seen": True, "plan": behavior}) + "\n")
        if behavior is None:
            continue
        time.sleep(delay)
        answer = {"id": req["id"], "behavior": behavior,
                  "message": f"Baton: the owner denied request {req['id']}."}
        tmp = os.path.join(ans_dir, f".{req['id']}.tmp")
        with open(tmp, "w") as f:
            json.dump(answer, f)
        os.rename(tmp, os.path.join(ans_dir, f"{req['id']}.json"))
        log.write(json.dumps({"ms": int(time.time() * 1000), "id": req["id"], "answered": behavior}) + "\n")
    time.sleep(0.2)
