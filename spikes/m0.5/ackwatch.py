#!/usr/bin/env python3
"""M0.5: log the time each ack-*.txt appears in <dir>. Usage: python3 ackwatch.py <dir> <seconds> <log>"""
import json
import os
import sys
import time

d, run_for, log = sys.argv[1], float(sys.argv[2]), sys.argv[3]
seen = set(f for f in os.listdir(d) if f.startswith("ack-"))
end = time.time() + run_for
while time.time() < end:
    for f in sorted(os.listdir(d)):
        if f.startswith("ack-") and f not in seen:
            seen.add(f)
            with open(log, "a") as out:
                out.write(json.dumps({"ms": int(time.time() * 1000), "ack": f}) + "\n")
    time.sleep(0.1)
