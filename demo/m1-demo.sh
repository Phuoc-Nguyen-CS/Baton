#!/usr/bin/env bash
# M1 demo: one reviewable task, end to end, with one real Claude Code worker.
#
#   intake -> permission decision (allow) -> worker's turn ends -> Baton's own
#   checks on the exact candidate -> TUI review screen -> live `claude attach`
#   and back -> request changes -> new candidate + checks -> accept -> evidence
#
# Spends 1 real session (Haiku) on the sandbox. Everything runs in a private
# tmux server so the TUI and attach get a real terminal; screens are captured
# to $OUT. Usage: demo/m1-demo.sh [--idle SECONDS]
#   --idle  wait this long at review before requesting changes (Claude Code
#           makes an `away_summary` request ~3 min into an idle session; 200
#           shows it as a side request in the usage cross-check)
set -euo pipefail

IDLE=0
[[ ${1:-} == --idle ]] && IDLE=${2:?seconds}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SANDBOX=${SANDBOX:-$HOME/projects/baton-sandbox}
export BATON_HOME=${BATON_HOME:-$HOME/.local/state/baton-smoke}
OUT=${OUT:-$ROOT/target/m1-demo/$(date +%Y%m%d-%H%M%S)}
TMUX_SOCK=baton-demo
mkdir -p "$OUT"

say() { printf '\n== %s\n' "$*"; }
tm() { tmux -L "$TMUX_SOCK" "$@"; }
snap() { sleep "${2:-1.5}"; tm capture-pane -p -t demo:tui > "$OUT/$1.txt"; echo "   screen -> $OUT/$1.txt"; }

say "build"
PATH=$HOME/.cargo/bin:$PATH cargo build --release --quiet --manifest-path "$ROOT/Cargo.toml"
BATON=$ROOT/target/release/baton
claude --version | tee "$OUT/claude-version.txt"

# `baton --json status`, then a Python expression over the task as `t`.
task_field() { "$BATON" --json status | python3 -c "
import json, sys
t = next(t for t in json.load(sys.stdin)['tasks'] if t['id'] == $TASK)
print($1)"; }
wait_state() { # state [timeout s]
  local deadline=$((SECONDS + ${2:-180}))
  until [[ $(task_field "t['state']") == "$1" ]]; do
    ((SECONDS < deadline)) || { echo "timed out waiting for $1"; "$BATON" status; exit 1; }
    sleep 1
  done
}

say "doctor"
(cd "$SANDBOX" && "$BATON" doctor)

say "daemon (private tmux server '$TMUX_SOCK')"
tm kill-server 2>/dev/null || true
tm new-session -d -s demo -n daemon -x 80 -y 24 "$BATON daemon 2>&1 | tee $OUT/daemon.log"
until "$BATON" status >/dev/null 2>&1; do sleep 0.2; done

say "intake"
cd "$SANDBOX"
"$BATON" --json task \
  "Create notes.txt containing the single line: one. Then run the Bash command python3 -c 'print(6*7)' and include its output in your summary." \
  --check "grep -qx one notes.txt" --check "test \$(wc -l < notes.txt) -ge 1" --model haiku | tee "$OUT/intake.json"
TASK=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["task"]["id"])' "$OUT/intake.json")
T0=$SECONDS

say "task $TASK: wait for the permission decision, then allow it"
wait_state waiting_permission 120
DECISION=$(task_field "t['decisions'][0]['id']")
task_field "t['decisions'][0]['summary']"
"$BATON" decide "$DECISION" allow

say "task $TASK: wait for review (worker turn + Baton's checks)"
wait_state review_ready 180
echo "   review_ready after $((SECONDS - T0)) s"
"$BATON" status

say "TUI at 80x24: list, review screen, live attach and back"
tm new-window -t demo -n tui "$BATON"
snap 01-list
tm send-keys -t demo:tui Enter; snap 02-review
tm send-keys -t demo:tui t;     snap 03-attached 6
tm send-keys -t demo:tui C-z;   snap 04-back 3
tm send-keys -t demo:tui q;     sleep 1
tm list-windows -t demo -F '#{window_name} alternate_on=#{alternate_on}' | tee "$OUT/05-after-quit.txt"
"$BATON" status | tee "$OUT/after-attach-status.txt"

if ((IDLE > 0)); then
  say "idle at review for $IDLE s"
  sleep "$IDLE"
  "$BATON" status
fi

say "request changes"
CAND1=$(task_field "t['candidate']['commit']")
"$BATON" review "$TASK" changes --candidate "$CAND1" --note "Add a second line to notes.txt: two."
sleep 2
wait_state review_ready 180
CAND2=$(task_field "t['candidate']['commit']")
[[ $CAND2 != "$CAND1" ]] || { echo "expected a new candidate"; exit 1; }

say "accept"
"$BATON" review "$TASK" accept --candidate "$CAND2"
sleep 2

say "evidence"
"$BATON" status | tee "$OUT/final-status.txt"
"$BATON" --json status > "$OUT/final-status.json"
git -C "$SANDBOX" log --format='%h %an %s' "baton/$TASK" -3 | tee "$OUT/branch.txt"
git -C "$SANDBOX" show "baton/$TASK:notes.txt" | tee "$OUT/notes.txt"
python3 - "$BATON_HOME/baton.db" "$TASK" <<'EOF' | tee "$OUT/decisions.txt"
import sqlite3, sys
db = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
print("human decisions:")
for kind, answer, status in db.execute(
        "SELECT kind, answer, status FROM decision WHERE task_id = ? ORDER BY id", (sys.argv[2],)):
    print(f"  {kind:<10} {answer or '-':<8} {status}")
EOF
tm kill-server
echo "wall time from intake: $((SECONDS - T0)) s; evidence in $OUT"
