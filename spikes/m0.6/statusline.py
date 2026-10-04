#!/usr/bin/env python3
"""M0.6 status-line command: log each JSON input to argv[1], print a one-line status."""
import json
import sys
import time

data = json.loads(sys.stdin.read() or "{}")
with open(sys.argv[1], "a") as f:
    f.write(json.dumps({"ms": int(time.time() * 1000), "input": data}) + "\n")
print("baton m0.6")
