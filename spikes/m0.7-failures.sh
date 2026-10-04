#!/usr/bin/env bash
# M0.7 failure-paths spike. See docs/compat-record.md C6, C8, C15.
#
# Usage: spikes/m0.7-failures.sh <step>
#   agent-setup    Baton-made worktree m7 with a project agent file (tools: Read, Write)
#   agent-spawn    worker A7 runs as that agent
#   agent-break    stop A7, delete the agent file
#   agent-resume   flag-free resume of A7: does it keep the role, warn, or widen?
#   report         hook events, tool calls and agent warnings per session
#   cleanup        stop and remove A7
#   supervisor     stops/kills the background supervisor. ENDS EVERY BACKGROUND SESSION
#                  on this machine unless --keep-workers holds; refuses unless CONFIRM=yes.
# Steps that start a Claude session log "BUDGET +1".
set -euo pipefail

SPIKES=$(cd "$(dirname "$0")" && pwd)
SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
WT_ROOT=${WT_ROOT:-$HOME/projects/baton-sandbox-worktrees}
OUT=${OUT:-$SPIKES/out/m0.7}
WT=$WT_ROOT/m7
AGENT_FILE=$WT/.claude/agents/m07-agent.md
source "$SPIKES/lib.sh"

case ${1:-} in
agent-setup)
  [[ -d $WT ]] || git -C "$SANDBOX" worktree add -q "$WT" -b baton/m7
  mkdir -p "$(dirname "$AGENT_FILE")"
  printf '%s\n' '---' 'name: m07-agent' 'description: Baton M0.7 test agent' 'tools: Read, Write' 'model: haiku' '---' \
    'You are a Baton M0.7 test agent. Do exactly the task you are given. End every final reply with AGENT7-CANARY.' > "$AGENT_FILE"
  python3 - "$SPIKES" "$OUT" > "$OUT/settings-a7.json" <<'PY'
import json, sys
spikes, out = sys.argv[1:]
logger = {"type": "command", "command": f"python3 {spikes}/m0.3/hook.py {out}/hooks.jsonl a7"}
print(json.dumps({"hooks": {e: [{"hooks": [logger]}] for e in ("SessionStart", "PreToolUse", "Stop", "SessionEnd")}}, indent=2))
PY
  log "agent-setup: $AGENT_FILE"
  ;;
agent-spawn)
  dispatch "$WT" "List the names of every tool you can call, comma-separated. Then reply DONE." \
    --name m07-a --model haiku --agent m07-agent --permission-mode acceptEdits --allowedTools "Bash(echo:*)" \
    --settings "$OUT/settings-a7.json" || exit 1
  r=$(row "$DISPATCHED_ID"); log "  row: $r"
  save A7_ID "$DISPATCHED_ID"; save A7_UUID "$(field "$r" sessionId)"
  ;;
agent-break)
  claude stop "$A7_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log"
  rm "$AGENT_FILE"
  log "agent-break: stopped $A7_ID, deleted $AGENT_FILE"
  ;;
agent-resume)
  dispatch "$WT" "List the names of every tool you can call now, comma-separated. If Bash is one of them, run: echo hi. Then reply DONE2." \
    --resume "$A7_UUID" || exit 1
  log "  same id: $([[ $DISPATCHED_ID == "$A7_ID" ]] && echo yes || echo "NO ($DISPATCHED_ID)")"
  ;;
observe)
  shift
  observe "$@"
  ;;
report)
  python3 - "$OUT" <<'PY'
import json, os, sys
out = sys.argv[1]
transcripts = set()
for line in open(os.path.join(out, "hooks.jsonl")):
    r = json.loads(line); i = r["input"] if isinstance(r["input"], dict) else {}
    transcripts.add(i.get("transcript_path"))
    e = i.get("hook_event_name")
    info = {"SessionStart": f"source={i.get('source')} agent_type={i.get('agent_type')}",
            "PreToolUse": f"{i.get('tool_name')} {(i.get('tool_input') or {}).get('command', '')}",
            "Stop": repr(i.get("last_assistant_message", ""))[:220],
            "SessionEnd": i.get("reason")}.get(e, "")
    print(f"{r['ms']} {i.get('session_id', '?')[:8]} {e:12} {info}")
print("== transcript entries mentioning the agent")
for t in filter(None, transcripts):
    for line in open(t):
        if "m07-agent" in line and '"type":"user"' not in line.replace(" ", ""):
            d = json.loads(line)
            print(" ", d.get("type"), json.dumps({k: v for k, v in d.items() if k in ("subtype", "content", "level", "agentSetting", "attachment")})[:300])
PY
  ;;
cleanup)
  claude stop "$A7_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  claude rm "$A7_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  ;;
supervisor)
  # Run from your own terminal when no other background session matters: both the
  # graceful stop and the kill can end every background session on the machine.
  [[ ${CONFIRM:-} == yes ]] || { echo "refusing: this stops the supervisor for ALL background sessions. Re-run with CONFIRM=yes."; exit 3; }
  sup_pid() { claude daemon status 2>&1 | sed -n 's/^pid: *\([0-9]*\).*/\1/p'; }
  alive() { kill -0 "$1" 2>/dev/null && echo alive || echo GONE; }
  log "supervisor: sentinel worker, busy for ~2 min"
  dispatch "$WT" "Run these Bash commands one at a time: sleep 30, sleep 30, sleep 30, sleep 30. Then reply DONE." \
    --name m07-sup --model haiku --permission-mode acceptEdits --allowedTools "Bash(sleep:*)" || exit 1
  S=$DISPATCHED_ID; save SUP_ID "$S"
  wait_state "$S" 'working' 30 || true
  timeout 30 bash -c "until claude agents --json | grep -q '\"pid\"'; do sleep 1; done" || true
  wpid=$(field "$(row "$S")" pid); d1=$(sup_pid)
  log "  worker $S pid=$wpid; supervisor pid=$d1"

  log "step 1: claude daemon stop --any --keep-workers"
  claude daemon stop --any --keep-workers 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  log "  supervisor $d1: $(alive "$d1"); worker $wpid: $(alive "$wpid")"
  log "step 2: claude agents --json (starts a supervisor on demand)"
  log "  row: $(row "$S")"; d2=$(sup_pid)
  log "  supervisor now pid=$d2; worker $wpid: $(alive "$wpid")"

  log "step 3: kill -9 supervisor $d2"
  kill -9 "$d2" 2>/dev/null || true
  timeout 10 bash -c "while kill -0 $d2 2>/dev/null; do sleep 0.5; done" || true
  log "  supervisor $d2: $(alive "$d2"); worker $wpid: $(alive "$wpid")"
  log "  row after kill (may restart the supervisor): $(row "$S")"; log "  supervisor now pid=$(sup_pid)"

  log "step 4: does the worker finish its task?"
  wait_state "$S" 'done|blocked|failed|stopped' 240 || true
  log "  final row: $(row "$S")"
  claude logs "$S" > "$OUT/logs-$S.txt" 2>&1 || true
  claude stop "$S" > /dev/null 2>&1 || true
  claude rm "$S" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  log "supervisor: done. Results are in $OUT/run.log"
  ;;
*) sed -n '2,14p' "$0"; exit 2 ;;
esac
