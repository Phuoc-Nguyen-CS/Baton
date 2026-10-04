# Shared helpers for spike scripts. Callers set SANDBOX and OUT, then source this file.
# Step state persists between invocations in $OUT/state.env.

STATE=$OUT/state.env
mkdir -p "$OUT"
touch "$STATE"
source "$STATE"

ms() { echo $(( $(date +%s%N) / 1000000 )); }  # uutils date ignores %3N
log() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "$OUT/run.log"; }
save() { echo "$1=$2" >> "$STATE"; }

# agents --json row for short id $1 (compact JSON, "{}" when absent)
row() {
  claude agents --json --all | python3 -c '
import json, sys
rows = [r for r in json.load(sys.stdin) if r.get("id") == sys.argv[1]]
print(json.dumps(rows[0] if rows else {}, sort_keys=True))' "$1"
}
field() { python3 -c 'import json, sys; print(json.loads(sys.argv[1]).get(sys.argv[2], ""))' "$1" "$2"; }

# wait until state matches regex $2; logs every state/status/waitingFor change
wait_state() {
  local id=$1 want=$2 deadline=$((SECONDS + ${3:-240})) last="" r s
  while (( SECONDS < deadline )); do
    r=$(row "$id")
    s="state=$(field "$r" state) status=$(field "$r" status) waitingFor=$(field "$r" waitingFor)"
    [[ $s != "$last" ]] && { log "  $id $s"; last=$s; }
    [[ $(field "$r" state) =~ ^($want)$ ]] && return 0
    sleep 2
  done
  log "  TIMEOUT: $id never reached ($want)"
  return 1
}

# run claude --bg in dir $1 with prompt $2, then flags; logs output, elapsed ms, short id.
# The prompt goes first: variadic flags such as --allowedTools would swallow it.
dispatch() {
  local dir=$1 prompt=$2; shift 2
  local t0 out rc
  t0=$(ms)
  out=$(cd "$dir" && claude --bg "$prompt" "$@" 2>&1) && rc=0 || rc=$?
  log "  exit=$rc elapsed_ms=$(( $(ms) - t0 ))"
  printf '%s\n' "$out" | sed 's/^/  | /' | tee -a "$OUT/run.log"
  # the id on the "backgrounded · <id>" line; a "note:" line may name another session first
  DISPATCHED_ID=$(printf '%s\n' "$out" | sed 's/\x1b\[[0-9;]*m//g' | grep -oE 'backgrounded · [0-9a-f]{8}' | grep -oE '[0-9a-f]{8}$' || true)
  [[ -n $DISPATCHED_ID ]] || { log "  no session started"; return 1; }
  log "  short id: $DISPATCHED_ID  BUDGET +1"
}

observe() {  # wait for each id to settle, record its row and logs
  local id
  for id in "$@"; do
    log "observe: $id"
    wait_state "$id" 'done|blocked|failed|stopped' || true
    log "  row: $(row "$id")"
    claude logs "$id" > "$OUT/logs-$id.txt" 2>&1 || true
  done
}

worktrees() { log "  worktrees of $SANDBOX:"; git -C "$SANDBOX" worktree list | sed 's/^/  | /' | tee -a "$OUT/run.log"; }
