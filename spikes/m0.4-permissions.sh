#!/usr/bin/env bash
# M0.4 permissions spike: answer a --bg worker's permission requests from a
# Baton-style inbox (allow, deny, no answer past the hook timeout), and block
# pushes with a recording PreToolUse hook. See docs/compat-record.md C9.
#
# Usage: spikes/m0.4-permissions.sh <step>
#   setup     Baton-made worktree e, settings with the hooks
#   spawn-e   start the stand-in responder, then worker E (default permission mode)
#   watch     log E's state every second until it waits on a native prompt or ends
#   recover   after claude stop: flag-free resume, allow the re-asked request via the inbox
#   report    merged timeline of hooks, inbox requests/answers, pushes blocked, tool results
#   cleanup   stop and remove E
# Steps that start a Claude session log "BUDGET +1".
set -euo pipefail

SPIKES=$(cd "$(dirname "$0")" && pwd)
SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
WT_ROOT=${WT_ROOT:-$HOME/projects/baton-sandbox-worktrees}
OUT=${OUT:-$SPIKES/out/m0.4}
INBOX=$OUT/inbox
HOOK_TIMEOUT=10   # seconds Claude Code gives the PermissionRequest hook
HOOK_WAIT=60      # seconds the hook would wait for an answer (longer, to hit the timeout)
ANSWER_DELAY=5    # seconds the stand-in Baton takes to answer
source "$SPIKES/lib.sh"

read -r -d '' PROMPT <<'EOF' || true
Use the Bash tool to run these four commands one at a time, in this order, and keep going even if one fails or is denied:
1. python3 -c "print('ALLOW-1')"
2. python3 -c "print('DENY-2')"
3. git push
4. python3 -c "print('WAIT-4')"
Then reply with one line per command saying what happened.
EOF

case ${1:-} in
setup)
  [[ -d $WT_ROOT/e ]] || git -C "$SANDBOX" worktree add -q "$WT_ROOT/e" -b baton/e
  python3 - "$SPIKES" "$OUT" "$INBOX" "$HOOK_TIMEOUT" "$HOOK_WAIT" > "$OUT/settings-e.json" <<'PY'
import json, sys
spikes, out, inbox, timeout, wait = sys.argv[1:]
logger = {"type": "command", "command": f"python3 {spikes}/m0.3/hook.py {out}/hooks.jsonl e"}
events = ["PreToolUse", "PermissionRequest", "PostToolUse", "PostToolUseFailure", "Notification", "Stop", "SessionEnd"]
hooks = {e: [{"hooks": [logger]}] for e in events}
hooks["PermissionRequest"].append({"hooks": [{"type": "command", "timeout": int(timeout),
    "command": f"python3 {spikes}/m0.4/perm_hook.py {inbox} {wait}"}]})
hooks["PreToolUse"].append({"matcher": "Bash", "hooks": [{"type": "command",
    "command": f"python3 {spikes}/m0.4/push_block.py {out}/push-blocked.jsonl"}]})
print(json.dumps({"hooks": hooks}, indent=2))
PY
  log "setup: worktree $WT_ROOT/e, settings $OUT/settings-e.json"
  ;;
spawn-e)
  nohup python3 "$SPIKES/m0.4/responder.py" "$INBOX" "$ANSWER_DELAY" 300 > /dev/null 2>&1 &
  log "spawn-e: responder pid $! (answers after ${ANSWER_DELAY}s, runs 300s)"
  dispatch "$WT_ROOT/e" "$PROMPT" --name m04-e --model haiku --permission-mode default \
    --settings "$OUT/settings-e.json" || exit 1
  r=$(row "$DISPATCHED_ID"); log "  row: $r"
  save E_ID "$DISPATCHED_ID"; save E_UUID "$(field "$r" sessionId)"
  ;;
watch)
  deadline=$((SECONDS + 300)); last=""
  while (( SECONDS < deadline )); do
    r=$(row "$E_ID")
    s="state=$(field "$r" state) status=$(field "$r" status) waitingFor=$(field "$r" waitingFor)"
    if [[ $s != "$last" ]]; then
      printf '%s\n' "{\"ms\": $(ms), \"row\": \"$s\"}" >> "$OUT/rows.jsonl"; log "  $E_ID $s"; last=$s
    fi
    [[ $(field "$r" waitingFor) == "permission prompt" || $(field "$r" state) =~ ^(done|failed|stopped)$ ]] && break
    sleep 1
  done
  ;;
recover)
  # after `claude stop`: resume with no flags, then allow the re-asked WAIT-4 request via the inbox
  python3 "$SPIKES/m0.4/allow_next.py" "$INBOX" WAIT-4 120 >> "$OUT/recover.jsonl" 2>&1 &
  log "recover: --resume $E_UUID, no flags; allow_next pid $!"
  dispatch "$WT_ROOT/e" "Continue: run the remaining command if it hasn't run, then reply with one line per command." \
    --resume "$E_UUID" || exit 1
  log "  same id: $([[ $DISPATCHED_ID == "$E_ID" ]] && echo yes || echo NO)"
  observe "$E_ID"
  ;;
report)
  python3 "$SPIKES/m0.4/timeline.py" "$OUT" | tee "$OUT/report.txt"
  ;;
cleanup)
  claude stop "$E_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  claude rm "$E_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  ;;
*) sed -n '2,13p' "$0"; exit 2 ;;
esac
