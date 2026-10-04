#!/usr/bin/env bash
# M0.5 steering spike: get an instruction to a running --bg worker, idle or busy,
# and measure delivery and acknowledgment. See docs/compat-record.md C10.
#
# Usage: spikes/m0.5-steering.sh <step>
#   setup        Baton-made worktree w, settings with logging + Baton-queue injection hooks
#   spawn-w      queue S1 for the Stop hook, start the ack watcher, start worker W (4 x sleep 4)
#   post-test    once W's first tool call finishes, queue P1 for the PostToolUse hook
#   mark <label> log a timestamp, e.g. just before a SendMessage
#   resume-idle  flag-free --resume of W while its process is alive and idle
#   observe <id>...  wait for sessions, record rows and logs
#   report       merged timeline: enqueue, delivery, hooks, acks, incoming messages
#   cleanup      stop and remove W (and any copy)
# Steps that start a Claude session log "BUDGET +1".
set -euo pipefail

SPIKES=$(cd "$(dirname "$0")" && pwd)
SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
WT_ROOT=${WT_ROOT:-$HOME/projects/baton-sandbox-worktrees}
OUT=${OUT:-$SPIKES/out/m0.5}
WT=$WT_ROOT/w
source "$SPIKES/lib.sh"

case ${1:-} in
setup)
  [[ -d $WT ]] || git -C "$SANDBOX" worktree add -q "$WT" -b baton/w
  python3 - "$SPIKES" "$OUT" > "$OUT/settings-w.json" <<'PY'
import json, sys
spikes, out = sys.argv[1:]
logger = {"type": "command", "command": f"python3 {spikes}/m0.3/hook.py {out}/hooks.jsonl w"}
events = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Notification", "Stop", "SessionEnd"]
hooks = {e: [{"hooks": [logger]}] for e in events}
for event, queue in (("PostToolUse", "queue-post"), ("Stop", "queue-stop")):
    hooks[event].append({"hooks": [{"type": "command",
        "command": f"python3 {spikes}/m0.5/inject.py {out}/{queue} {out}/steer.jsonl"}]})
# accept cross-session messages unattended, as the docs advise for workers nobody watches
print(json.dumps({"hooks": hooks, "crossSessionInbound": "accept"}, indent=2))
PY
  log "setup: worktree $WT, settings $OUT/settings-w.json"
  ;;
spawn-w)
  python3 "$SPIKES/m0.5/enqueue.py" "$OUT/queue-stop" S1 "$OUT/steer.jsonl"
  nohup python3 "$SPIKES/m0.5/ackwatch.py" "$WT" 1200 "$OUT/acks.jsonl" > /dev/null 2>&1 &
  log "spawn-w: S1 queued for Stop; ack watcher pid $!"
  dispatch "$WT" "Run these Bash commands one at a time, in order: sleep 4, sleep 4, sleep 4, sleep 4. Then reply DONE. If you get a message from Baton, do what it asks, then carry on." \
    --name m05-w --model haiku --permission-mode acceptEdits --allowedTools "Bash(sleep:*)" \
    --settings "$OUT/settings-w.json" || exit 1
  r=$(row "$DISPATCHED_ID"); log "  row: $r"
  save W_ID "$DISPATCHED_ID"; save W_UUID "$(field "$r" sessionId)"
  ;;
post-test)
  timeout 120 bash -c "until grep -q '\"PostToolUse\"' '$OUT/hooks.jsonl' 2>/dev/null; do sleep 0.2; done"
  python3 "$SPIKES/m0.5/enqueue.py" "$OUT/queue-post" P1 "$OUT/steer.jsonl"
  log "post-test: P1 queued after W's first PostToolUse"
  ;;
mark)
  echo "{\"ms\": $(ms), \"mark\": \"$2\"}" >> "$OUT/steer.jsonl"
  log "mark: $2"
  ;;
resume-idle)
  r=$(row "$W_ID"); log "resume-idle: W row before: $r"
  echo "{\"ms\": $(ms), \"mark\": \"resume-idle R1\"}" >> "$OUT/steer.jsonl"
  dispatch "$WT" "Create a file named ack-R1.txt containing R1." --resume "$W_UUID" || exit 1
  log "  same id: $([[ $DISPATCHED_ID == "$W_ID" ]] && echo yes || echo "NO (copy $DISPATCHED_ID)")"
  save R_ID "$DISPATCHED_ID"
  ;;
stop-resume)
  # idle steering without a Claude sender: stop W, then wake it with the instruction as its prompt
  echo "{\"ms\": $(ms), \"mark\": \"stop-resume R2: stop\"}" >> "$OUT/steer.jsonl"
  claude stop "$W_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log"
  echo "{\"ms\": $(ms), \"mark\": \"stop-resume R2: resume\"}" >> "$OUT/steer.jsonl"
  dispatch "$WT" "Message from Baton (R2): create a file named ack-R2.txt containing R2, then reply DONE-R2." --resume "$W_UUID" || exit 1
  log "  same id: $([[ $DISPATCHED_ID == "$W_ID" ]] && echo yes || echo "NO (copy $DISPATCHED_ID)")"
  ;;
observe)
  shift
  observe "$@"
  ;;
report)
  python3 "$SPIKES/m0.5/timeline.py" "$OUT" | tee "$OUT/report.txt"
  ;;
cleanup)
  for id in ${W_ID:-} ${R_ID:-}; do
    claude stop "$id" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
    claude rm "$id" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  done
  ;;
*) sed -n '2,15p' "$0"; exit 2 ;;
esac
