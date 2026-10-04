#!/usr/bin/env python3
"""M0.3 minimal stdio MCP server with one tool, `ping`. Logs each message's method to argv[1]."""
import json
import sys
import time

log = open(sys.argv[1], "a", buffering=1)


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method, mid = msg.get("method"), msg.get("id")
    log.write(json.dumps({"ms": int(time.time() * 1000), "method": method, "id": mid}) + "\n")
    if mid is None:  # notification, e.g. notifications/initialized
        continue
    if method == "initialize":
        result = {
            "protocolVersion": msg["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "baton-spike", "version": "0.1"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "ping",
            "description": "Returns pong and a timestamp. Used by the Baton M0.3 spike.",
            "inputSchema": {"type": "object", "properties": {}},
        }]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": f"pong {int(time.time())}"}]}
    elif method == "ping":
        result = {}
    else:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": f"unknown method {method}"}})
        continue
    send({"jsonrpc": "2.0", "id": mid, "result": result})
