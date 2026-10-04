#!/usr/bin/env python3
"""Write the M0.3 per-session config files into OUT (argv[1]).

agents-c.json      custom agent with a canary in its prompt
mcp-c.json         the ping MCP server
settings-*.json    logging hooks for every relevant event (+ deny rules where noted)
"""
import json
import os
import sys

out = os.path.abspath(sys.argv[1])  # hooks and the MCP server run from the worker's cwd
here = os.path.dirname(os.path.abspath(__file__))
hook = os.path.join(here, "hook.py")
hooks_log = os.path.join(out, "hooks.jsonl")
EVENTS = ["SessionStart", "InstructionsLoaded", "PreToolUse", "PermissionRequest",
          "PermissionDenied", "Notification", "Stop", "SessionEnd"]
DENY = ["Bash(git push:*)", "Bash(gh pr:*)"]


def hooks(label):
    cmd = f"python3 {hook} {hooks_log} {label}"
    return {e: [{"hooks": [{"type": "command", "command": cmd}]}] for e in EVENTS}


def write(name, data):
    with open(os.path.join(out, name), "w") as f:
        json.dump(data, f, indent=2)


def agent(tools):
    return {"baton-worker": {
        "description": "Baton M0.3 test worker",
        "prompt": "You are a Baton test worker. Do exactly the task you are given, nothing more. "
                  "End every final reply with the token AGENT-CANARY.",
        "tools": tools,
        "model": "haiku",
    }}


write("agents-c.json", agent(["Read", "Write", "Edit", "Bash", "mcp__baton__ping"]))
# c2: MCP tools are deferred behind tool search by default, so the agent needs ToolSearch
write("agents-c2.json", agent(["Read", "Write", "Edit", "Bash", "ToolSearch", "mcp__baton__ping"]))
write("mcp-c.json", {"mcpServers": {"baton": {
    "type": "stdio", "command": "python3",
    "args": [os.path.join(here, "mcp_ping.py"), os.path.join(out, "mcp.log")],
}}})
write("settings-c.json", {"hooks": hooks("c"), "permissions": {"deny": DENY}})
write("settings-d-control.json", {"hooks": hooks("d-control")})
write("settings-d-deny.json", {"hooks": hooks("d-deny"), "permissions": {"deny": DENY}})
write("project-settings.json", {"hooks": {"SessionStart": [{"hooks": [
    {"type": "command", "command": f"python3 {hook} {hooks_log} project"}]}]}})
