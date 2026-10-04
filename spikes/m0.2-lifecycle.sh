#!/usr/bin/env bash
# M0.2 lifecycle spike: spawn -> list -> logs -> stop -> resume, plus dispatch
# inside a Baton-made worktree. See docs/compat-record.md C1-C5.
#
# Usage: spikes/m0.2-lifecycle.sh <step>
#   spawn-a    start session A (preassigned --session-id) in the sandbox
#   observe-a  wait for A, record row, logs, worktrees
#   stop-a     stop A
#   resume-a   resume A by UUID with --bg plus flags (2.1.289: starts copies)
#   observe-a2 wait for the resumed sessions, record rows, logs, file contents
#   resume-plain resume A with no flags (continues A), then again while it runs (copy)
#   observe <id>...  wait for any sessions, record rows and logs
#   spawn-b    create a worktree ourselves and dispatch session B inside it
#   observe-b  wait for B, check for nested worktrees
#   cleanup    claude rm every session, record what happens to worktrees
# Steps that start a Claude session log "BUDGET +1".
set -euo pipefail

SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
WT_ROOT=${WT_ROOT:-$HOME/projects/baton-sandbox-worktrees}
OUT=${OUT:-$(cd "$(dirname "$0")" && pwd)/out/m0.2}
MODEL=haiku
ALLOW="Bash(git add:*) Bash(git commit:*) Bash(git status:*) Bash(git log:*) Bash(git diff:*)"
source "$(dirname "$0")/lib.sh"

case ${1:-} in
spawn-a)
  uuid=$(cat /proc/sys/kernel/random/uuid)
  log "spawn-a: --session-id $uuid"
  dispatch "$SANDBOX" "Create a file named hello.txt containing the line: hi. Then reply with the single word DONE." \
    --name m02-a --session-id "$uuid" --model $MODEL --permission-mode acceptEdits --allowedTools "$ALLOW" || exit 1
  r=$(row "$DISPATCHED_ID"); log "  row: $r"
  # 2.1.289: --bg warns and ignores --session-id, so take the real one from the row
  save A_UUID "$(field "$r" sessionId)"; save A_ID "$DISPATCHED_ID"
  log "  preassigned honored: $([[ $(field "$r" sessionId) == "$uuid" ]] && echo yes || echo NO)"
  ;;
observe-a)
  log "observe-a: $A_ID"
  wait_state "$A_ID" 'done|blocked|failed|stopped' || true
  r=$(row "$A_ID"); log "  row: $r"
  log "  sessionId matches preassigned: $([[ $(field "$r" sessionId) == "$A_UUID" ]] && echo yes || echo NO)"
  claude logs "$A_ID" > "$OUT/a-logs.txt" 2>&1 || true
  log "  logs: $(wc -l < "$OUT/a-logs.txt") lines -> a-logs.txt"
  worktrees
  log "  hello.txt files: $(find "$SANDBOX" -name hello.txt 2>/dev/null | tr '\n' ' ')"
  ;;
stop-a)
  log "stop-a: $A_ID"
  claude stop "$A_ID" 2>&1 | sed 's/^/  | /' | tee -a "$OUT/run.log"
  wait_state "$A_ID" 'stopped|done|failed' 60 || true
  log "  row: $(row "$A_ID")"
  ;;
resume-a)
  log "resume-a: --resume $A_UUID --bg"
  dispatch "$SANDBOX" "Append a second line containing: again, to hello.txt. Then reply with the single word DONE2." \
    --resume "$A_UUID" --model $MODEL --permission-mode acceptEdits --allowedTools "$ALLOW" || exit 1
  save A2_ID "$DISPATCHED_ID"
  log "  row: $(row "$DISPATCHED_ID")"
  log "resume-a (while running): --resume $A_UUID --bg"
  if dispatch "$SANDBOX" "Reply with the single word PING." --resume "$A_UUID" --model $MODEL; then
    save A3_ID "$DISPATCHED_ID"
    log "  row: $(row "$DISPATCHED_ID")"
  fi
  ;;
observe-a2)
  for id in "$A2_ID" "$A3_ID"; do
    log "observe-a2: $id"
    wait_state "$id" 'done|blocked|failed|stopped' || true
    log "  row: $(row "$id")"
    claude logs "$id" > "$OUT/logs-$id.txt" 2>&1 || true
  done
  worktrees
  for f in $(find "$SANDBOX" -name hello.txt 2>/dev/null); do log "  $f:"; sed 's/^/  | /' "$f" | tee -a "$OUT/run.log"; done
  ;;
resume-plain)
  # 2.1.289: any flag with --resume starts a copy; only the prompt should continue A itself
  log "resume-plain: --resume $A_UUID --bg, no flags"
  dispatch "$SANDBOX" "Reply with the single word PONG." --resume "$A_UUID" || exit 1
  save A4_ID "$DISPATCHED_ID"
  log "  continued A under the same id: $([[ $DISPATCHED_ID == "$A_ID" ]] && echo yes || echo NO)"
  log "resume-plain (while running): --resume $A_UUID --bg, no flags"
  if dispatch "$SANDBOX" "Reply with the single word PONG2." --resume "$A_UUID"; then
    save A5_ID "$DISPATCHED_ID"
  fi
  ;;
observe)
  # observe <id>...: wait for each, record row and logs
  shift
  for id in "$@"; do
    log "observe: $id"
    wait_state "$id" 'done|blocked|failed|stopped' || true
    log "  row: $(row "$id")"
    claude logs "$id" > "$OUT/logs-$id.txt" 2>&1 || true
  done
  ;;
spawn-b)
  # Baton-made worktrees: one inside the repo (where Claude puts its own), one in a
  # sibling directory. A location the trust check refuses starts no session.
  for wt in "$SANDBOX/.claude/worktrees/baton-b1" "$WT_ROOT/b2"; do
    git -C "$SANDBOX" worktree add -q "$wt" -b "baton/$(basename "$wt")"
    log "spawn-b: dispatch inside $wt"
    if dispatch "$wt" "Create a file named b.txt containing the line: b. Then reply with the single word DONE." \
        --name "m02-$(basename "$wt")" --model $MODEL --permission-mode acceptEdits --allowedTools "$ALLOW"; then
      save "ID_$(basename "$wt" | tr - _)" "$DISPATCHED_ID"
      log "  row: $(row "$DISPATCHED_ID")"
    fi
  done
  ;;
observe-b)
  for id in ${ID_baton_b1:-} ${ID_b2:-}; do
    log "observe-b: $id"
    wait_state "$id" 'done|blocked|failed|stopped' || true
    log "  row: $(row "$id")"
    claude logs "$id" > "$OUT/logs-$id.txt" 2>&1 || true
  done
  worktrees
  log "  b.txt files: $(find "$SANDBOX" "$WT_ROOT" -name b.txt 2>/dev/null | tr '\n' ' ')"
  ;;
cleanup)
  for id in ${A_ID:-} ${A2_ID:-} ${A3_ID:-} ${A5_ID:-} ${ID_baton_b1:-} ${ID_b2:-}; do
    log "cleanup: claude rm $id"
    claude rm "$id" 2>&1 | sed 's/^/  | /' | tee -a "$OUT/run.log" || true
  done
  worktrees
  ;;
*) sed -n '2,16p' "$0"; exit 2 ;;
esac
