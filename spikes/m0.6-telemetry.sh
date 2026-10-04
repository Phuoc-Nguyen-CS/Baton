#!/usr/bin/env bash
# M0.6 telemetry spike: what usage/quota data Baton can read from a detached --bg
# worker, from OpenTelemetry, the status line and the transcript. See compat record C12.
#
# Usage: spikes/m0.6-telemetry.sh <step>
#   setup     Baton-made worktree w6; settings with OTLP env, a logging status line, hooks
#   spawn     start the OTLP receiver, then worker W6 (2 x sleep 3 + one file write)
#   settle    wait until 15 s after W6's last Stop, so the final exports arrive
#   resume    stop W6, resume it with no flags and one more prompt (second process)
#   compare   tokens/cost per source and session; status-line and quota fields
#   cleanup   stop and remove W6, stop the receiver
# Steps that start a Claude session log "BUDGET +1".
set -euo pipefail

SPIKES=$(cd "$(dirname "$0")" && pwd)
SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
WT_ROOT=${WT_ROOT:-$HOME/projects/baton-sandbox-worktrees}
OUT=${OUT:-$SPIKES/out/m0.6}
WT=$WT_ROOT/w6
PORT=${PORT:-43176}
source "$SPIKES/lib.sh"

case ${1:-} in
setup)
  [[ -d $WT ]] || git -C "$SANDBOX" worktree add -q "$WT" -b baton/w6
  python3 - "$SPIKES" "$OUT" "$PORT" > "$OUT/settings-w6.json" <<'PY'
import json, sys
spikes, out, port = sys.argv[1:]
logger = {"type": "command", "command": f"python3 {spikes}/m0.3/hook.py {out}/hooks.jsonl w6"}
print(json.dumps({
    # project/local settings can only turn OTel off; --settings may turn it on
    "env": {"CLAUDE_CODE_ENABLE_TELEMETRY": "1", "OTEL_METRICS_EXPORTER": "otlp", "OTEL_LOGS_EXPORTER": "otlp",
            "OTEL_EXPORTER_OTLP_PROTOCOL": "http/json", "OTEL_EXPORTER_OTLP_ENDPOINT": f"http://127.0.0.1:{port}",
            "OTEL_METRIC_EXPORT_INTERVAL": "5000", "OTEL_LOGS_EXPORT_INTERVAL": "2000"},
    "statusLine": {"type": "command", "command": f"python3 {spikes}/m0.6/statusline.py {out}/statusline.jsonl",
                   "refreshInterval": 5},
    "hooks": {e: [{"hooks": [logger]}] for e in ("SessionStart", "Stop", "SessionEnd")},
}, indent=2))
PY
  log "setup: worktree $WT, settings $OUT/settings-w6.json, OTLP port $PORT"
  ;;
spawn)
  nohup python3 "$SPIKES/m0.6/otlp_receiver.py" "$PORT" "$OUT" 1500 > "$OUT/receiver.log" 2>&1 &
  save RECEIVER_PID "$!"
  log "spawn: OTLP receiver pid $! on 127.0.0.1:$PORT"
  dispatch "$WT" "Run these Bash commands one at a time: sleep 3, sleep 3. Then create a file t.txt containing t. Then reply DONE." \
    --name m06-w --model haiku --permission-mode acceptEdits --allowedTools "Bash(sleep:*)" \
    --settings "$OUT/settings-w6.json" || exit 1
  r=$(row "$DISPATCHED_ID"); log "  row: $r"
  save W6_ID "$DISPATCHED_ID"; save W6_UUID "$(field "$r" sessionId)"
  ;;
settle)
  n=${2:-1}  # wait for this many Stop events in total
  timeout 180 bash -c "until [ \"\$(cat '$OUT/hooks.jsonl' 2>/dev/null | grep -c '\"Stop\"')\" -ge $n ]; do sleep 1; done"
  log "settle: $n Stop event(s) seen; waiting 15 s for final exports"
  timeout 20 bash -c "t=\$(date +%s); until (( \$(date +%s) - t >= 15 )); do sleep 1; done"
  log "settle: done"
  ;;
resume)
  claude stop "$W6_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log"
  dispatch "$WT" "Reply with the single word AGAIN." --resume "$W6_UUID" || exit 1
  log "  same id: $([[ $DISPATCHED_ID == "$W6_ID" ]] && echo yes || echo "NO ($DISPATCHED_ID)")"
  ;;
compare)
  python3 "$SPIKES/m0.6/compare.py" "$OUT" | tee "$OUT/report.txt"
  ;;
cleanup)
  claude stop "$W6_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  claude rm "$W6_ID" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  kill "${RECEIVER_PID:-0}" 2>/dev/null && log "cleanup: receiver stopped" || true
  ;;
*) sed -n '2,13p' "$0"; exit 2 ;;
esac
