#!/usr/bin/env python3
"""M0.6 minimal OTLP/HTTP-JSON receiver on 127.0.0.1:<port>.

Appends {ms, path, body} per POST to <out>/otlp.jsonl and answers 200 {}.
Usage: python3 otlp_receiver.py <port> <out_dir> <seconds>
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, out, run_for = int(sys.argv[1]), sys.argv[2], float(sys.argv[3])
log = open(f"{out}/otlp.jsonl", "a", buffering=1)


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        try:
            body = json.loads(raw)
        except ValueError:
            body = {"undecodable_bytes": len(raw), "content_type": self.headers.get("Content-Type")}
        log.write(json.dumps({"ms": int(time.time() * 1000), "path": self.path, "body": body}) + "\n")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, *args):
        pass


server = HTTPServer(("127.0.0.1", port), Handler)
threading.Timer(run_for, server.shutdown).start()
server.serve_forever()
