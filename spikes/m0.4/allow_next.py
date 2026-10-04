#!/usr/bin/env python3
"""M0.4: act as the owner answering through Baton. Allow the next new request in <inbox>
whose command contains <text>, within <seconds>. Ignores requests that existed at start."""
import json
import os
import sys
import time

inbox, text, run_for = sys.argv[1], sys.argv[2], float(sys.argv[3])
req_dir, ans_dir = os.path.join(inbox, "requests"), os.path.join(inbox, "answers")
before = set(os.listdir(req_dir))
end = time.time() + run_for
while time.time() < end:
    for name in sorted(set(os.listdir(req_dir)) - before):
        if not name.endswith(".json"):
            continue
        req = json.load(open(os.path.join(req_dir, name)))
        if text in json.dumps(req.get("tool_input")):
            with open(os.path.join(ans_dir, f".{req['id']}.tmp"), "w") as f:
                json.dump({"id": req["id"], "behavior": "allow"}, f)
            os.rename(os.path.join(ans_dir, f".{req['id']}.tmp"), os.path.join(ans_dir, f"{req['id']}.json"))
            print(json.dumps({"ms": int(time.time() * 1000), "allowed": req["id"]}))
            sys.exit(0)
    time.sleep(0.2)
print("no matching request")
sys.exit(1)
