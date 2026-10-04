#!/usr/bin/env python3
"""M0.5: queue an instruction asking the worker to write ack-<id>.txt. Logs the enqueue time.
Usage: python3 enqueue.py <queue_dir> <id> <log>"""
import json
import os
import sys
import time

queue, mid, log = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(queue, exist_ok=True)
ms = int(time.time() * 1000)
msg = {"id": mid, "text": f"Create a file named ack-{mid}.txt containing {mid}, then carry on with your task."}
tmp = os.path.join(queue, f".{mid}.tmp")
with open(tmp, "w") as f:
    json.dump(msg, f)
os.rename(tmp, os.path.join(queue, f"{ms}-{mid}.json"))
with open(log, "a") as f:
    f.write(json.dumps({"ms": ms, "enqueued": mid, "queue": os.path.basename(queue)}) + "\n")
