#!/usr/bin/env bash
# M0.3 per-session config spike: custom agent + hooks + MCP server on one --bg
# session, user/project instructions still loaded, and push deny rules.
# See docs/compat-record.md C7, C8 and finding 2.
#
# Usage: spikes/m0.3-config.sh <step>
#   setup     project CLAUDE.md + settings in the sandbox, local bare origin, config files
#   spawn-c [c|c2]  worker in a Baton-made worktree with --agents/--agent/--settings/--mcp-config (c2 adds ToolSearch)
#   resume-c  resume C with no flags: do agent, hooks and MCP come back?
#   spawn-d   D-control (git push allowed) and D-deny (same + deny rule), Claude-made worktrees
#   observe <id>...  wait for sessions, record rows and logs
#   report    summarize hooks, MCP calls, files and the remote's branches
#   cleanup   claude rm every session
# Steps that start a Claude session log "BUDGET +1".
set -euo pipefail

SPIKES=$(cd "$(dirname "$0")" && pwd)
SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
WT_ROOT=${WT_ROOT:-$HOME/projects/baton-sandbox-worktrees}
REMOTE=${REMOTE:-$HOME/projects/baton-sandbox-remote.git}
OUT=${OUT:-$SPIKES/out/m0.3}
MODEL=haiku
GIT_ALLOW="Bash(git add:*) Bash(git commit:*) Bash(git status:*) Bash(git log:*) Bash(git diff:*) Bash(git push:*) Bash(git branch:*) Bash(git remote:*) Bash(git rev-parse:*)"
source "$SPIKES/lib.sh"

case ${1:-} in
setup)
  python3 "$SPIKES/m0.3/make_config.py" "$OUT"
  mkdir -p "$SANDBOX/.claude"
  cp "$OUT/project-settings.json" "$SANDBOX/.claude/settings.json"
  printf '%s\n' "# Sandbox project instructions" "" \
    "When you give your final reply, also include the token PROJECT-CANARY." > "$SANDBOX/CLAUDE.md"
  printf '.claude/worktrees/\n' > "$SANDBOX/.gitignore"
  git -C "$SANDBOX" add CLAUDE.md .gitignore .claude/settings.json
  git -C "$SANDBOX" commit -q -m "M0.3 project instructions and settings"
  [[ -d $REMOTE ]] || git init -q --bare "$REMOTE"
  git -C "$SANDBOX" remote get-url origin >/dev/null 2>&1 || git -C "$SANDBOX" remote add origin "$REMOTE"
  git -C "$SANDBOX" push -q -u origin main
  git -C "$SANDBOX" worktree add -q "$WT_ROOT/c" -b baton/c
  log "setup: project files committed, origin=$REMOTE, worktree $WT_ROOT/c"
  ls "$OUT"/*.json | sed 's/^/  | /'
  ;;
spawn-c)
  # spawn-c [c|c2]: c2's agent also lists ToolSearch
  v=${2:-c}
  [[ -d $WT_ROOT/$v ]] || git -C "$SANDBOX" worktree add -q "$WT_ROOT/$v" -b "baton/$v"
  log "spawn-c: agent + hooks + MCP in $WT_ROOT/$v"
  # 2.1.289: --bg rejects the file form of --agents, so pass the JSON inline
  dispatch "$WT_ROOT/$v" "Call the ping tool from the baton MCP server once. Write the exact text it returned into a new file c.txt. Then reply DONE." \
    --name "m03-$v" --model $MODEL --permission-mode acceptEdits --allowedTools "mcp__baton__ping" \
    --agents "$(cat "$OUT/agents-$v.json")" --agent baton-worker --settings "$OUT/settings-c.json" \
    --mcp-config "$OUT/mcp-c.json" || exit 1
  r=$(row "$DISPATCHED_ID"); log "  row: $r"
  save "${v^^}_ID" "$DISPATCHED_ID"; save "${v^^}_UUID" "$(field "$r" sessionId)"
  ;;
resume-c)
  # resume-c [c|c2]; stop the session first, or resuming makes a copy
  v=${2:-c}; id_var="${v^^}_ID"; uuid_var="${v^^}_UUID"
  log "resume-c: --resume ${!uuid_var}, no flags"
  # first run: Haiku guessed `Bash: baton call ping` instead of the MCP tool, so name it exactly
  dispatch "$WT_ROOT/$v" "Call the MCP tool named mcp__baton__ping (not Bash). Write the exact text it returned into a new file c.txt. Then reply DONE2." \
    --resume "${!uuid_var}" || exit 1
  log "  same id: $([[ $DISPATCHED_ID == "${!id_var}" ]] && echo yes || echo NO)"
  ;;
spawn-d)
  for variant in control deny; do
    log "spawn-d: $variant (git push allowed$([[ $variant == deny ]] && echo ', deny rule set'))"
    if dispatch "$SANDBOX" "Create a file named d.txt containing the line: d. Then reply with the single word DONE." \
        --name "m03-d-$variant" --model $MODEL --permission-mode acceptEdits --allowedTools "$GIT_ALLOW" \
        --settings "$OUT/settings-d-$variant.json"; then
      save "D_${variant^^}_ID" "$DISPATCHED_ID"
    fi
  done
  ;;
observe)
  shift
  observe "$@"
  ;;
report)
  python3 "$SPIKES/m0.3/summarize.py" "$OUT/hooks.jsonl" "$OUT/mcp.log" | tee "$OUT/report.txt"
  echo "== files"; for f in "$WT_ROOT/c/c.txt" $(find "$SANDBOX/.claude/worktrees" -name d.txt 2>/dev/null); do echo "  $f:"; sed 's/^/  | /' "$f"; done
  echo "== remote branches"; git ls-remote --heads "$REMOTE" | sed 's/^/  | /'
  ;;
cleanup)
  for id in ${C_ID:-} ${C2_ID:-} ${D_CONTROL_ID:-} ${D_DENY_ID:-}; do
    log "cleanup: claude rm $id"
    claude rm "$id" 2>&1 | sed 's/\x1b\[[0-9;]*m//g; s/^/  | /' | tee -a "$OUT/run.log" || true
  done
  worktrees
  ;;
*) sed -n '2,14p' "$0"; exit 2 ;;
esac
