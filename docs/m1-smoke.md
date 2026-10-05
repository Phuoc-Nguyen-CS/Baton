# M1 smoke runs

Real Claude Code sessions run by Baton during M1 (budget: ≤30 Haiku sessions, docs/decisions.md). Claude Code 2.1.289, WSL2, sandbox `~/projects/baton-sandbox`, state dir `~/.local/state/baton-smoke`. Evidence was collected with the job's helper scripts (status JSON, `last-message.txt`, check logs, the audit table, `claude agents --json --all`, git).

| # | Date | Slice | Task | Sessions | Result |
|---|---|---|---|---|---|
| 1 | 2026-10-04 | M1.3 + M1.5 | 1: "Create greeting.txt … Hello from Baton. Use only the Write tool." `--check "grep -qx 'Hello from Baton' greeting.txt"` `--model haiku` | 1 | ✅ `review_ready`, 1 of 1 checks passed |
| 2 | 2026-10-04 | M1.4 | 2: "Run these three Bash commands one at a time … print(101) … print(102) … print(103). If a command is denied, don't retry it" `--check "test -f README.md"` `--model haiku` | 1 | ✅ allow, deny and late allow all delivered; `review_ready` |
| 3 | 2026-10-04 | M1.6 | 3: "Create a file named usage.txt containing the single word ok. Use only the Write tool." `--check "grep -qx ok usage.txt"` `--model haiku` | 1 | ✅ OTel totals = transcript; quota seen; no identity stored. Cross-check read too early (fixed) |

## Run 1: one worker end to end (M1.3, M1.5)

**Tested (2.1.289):**
- Dispatch from the Baton-made worktree `.claude/worktrees/baton-1-1` (branch `baton/1` at `9686e63`) returned session `47026c08` in 0.74 s.
- The generated hook command `BATON_HOME='…' '…/baton' hook --attempt 1 --role baton-worker <Event>` ran from `--settings`. `SessionStart` reported `agent_type: baton-worker` and model `claude-haiku-4-5-20251001`; the role check passed. `PreToolUse` reported `Write`.
- The inline agent (tools Read, Write, Edit, Bash, Glob, Grep) was accepted. The worker used only Write and needed no permission prompt in `acceptEdits`.
- The worker ended with the handoff format from Baton's agent prompt (`STATUS: done`, `SUMMARY`, `CHANGED`, `CHECKS`, `OPEN`). The sandbox's project `CLAUDE.md` still loaded: the reply carried its `PROJECT-CANARY` token.
- The `Stop` hook arrived 9.0 s after intake. Baton snapshotted candidate `ce23d61` (author `Baton`, only `greeting.txt` added) on `baton/1`, ran the check (exit 0) and reached `review_ready` at 9.55 s.
- The worker did not commit itself (as in C4). The main checkout stayed clean.
- Claude's row read `state: done`, `status: idle`. After `claude stop 47026c08`, the `SessionEnd` hook (reason `other`) arrived and polling showed `not_running`.

**Found and fixed:** an activity note ("worker session ended") overwrote the review summary. Activity now updates the reason only while a task is `running`.

## Run 2: permissions, allow / deny / late answer (M1.4)

A driver script played the owner: it answered each decision through `baton decide` as soon as it appeared, except the third, which it answered only after Baton's 50 s hook wait had run out.

**Tested (2.1.289):**
- Each `python3 -c …` call reached Baton's `PermissionRequest` hook (`acceptEdits` doesn't cover Bash). None matched the M1 policy, so each became a durable decision, and the task showed `waiting_permission` with the exact `baton decide` command.
- **Allow:** decision 1 was answered 0.52 s after it was created, the waiting hook returned `allow`, and the command ran (output `101`).
- **Deny:** decision 2 was answered in 0.57 s with a note. The worker received exactly that text ("Denied by the owner for this smoke test.") and didn't retry.
- **Late answer:** decision 3 went unanswered past the hook's wait. Claude's own prompt stayed up (`state: blocked`, `status: waiting`, `waitingFor: permission prompt`), as in F16. `baton decide 3 allow`, given 63 s after the request:
  - Baton confirmed the worker was still at the prompt, ran `claude stop`, waited for the process to go, and resumed flag-free from the worktree. The session id stayed `71155bb2`: no copy.
  - `SessionStart` reported `source: resume` with the role intact.
  - The transcript recorded the original call as "Tool call interrupted … outcome unknown" (as in C9), and the worker retried the identical call (same command and description).
  - The retry matched decision 3 and the command ran (output `103`). From answer to applied took 5.2 s.
- The worker's own summary matched the evidence (101 ok, 102 denied, 103 ok). The candidate is the base commit, since no files changed, and its check passed.

**Found and fixed before this run succeeded:**
- **Capacity bug:** task 2 first sat `queued` for 21 minutes with no reason, because task 1's idle worker (turn ended, awaiting review) was counted as busy. No session was spent. Now only `preparing`/`dispatching`/`running` attempts hold the slot, and queued tasks say what they're waiting for.
- **Deadlock in that fix:** a `MutexGuard` in a `match` scrutinee lived through the match arms. The fake-backend tests hung on it before it reached a real run.

## Run 3: usage and quota (M1.6)

**Tested (2.1.289):**
- The worker's `--settings` `env` sent OTLP/HTTP-JSON to Baton's receiver on `127.0.0.1:47318`. Baton stored 2 `api_request` rows, keyed by `request_id` and matched to session `411afd47` by UUID.
- **OTel totals:** 18 input, 422 output, 11,627 cache read, 2,851 cache write, $0.0090 (client estimate).
- **Transcript, counted independently** with the M0.6 Python logic after the run: 2 messages from 4 entries, the same 18 / 422 / 11,627 / 2,851. The totals match exactly.
- **Quota** reached Baton through the status line hook (`refreshInterval: 5`), showing 5 h 85% and 7 d 22% one second after the turn. It is account-wide: it includes the owner's other sessions, mostly this build session.
- The owner's email doesn't appear anywhere in `baton.db`, although every OTel event carries it.

**Found and fixed:** Baton read the transcript at the `Stop` hook. At that moment the turn's last assistant message wasn't in the file yet, so the stored cross-check held only the first message (10 / 218 / 4,831 / 1,965) and `status` said "transcript differs". Baton now re-reads the transcript on each poll for 60 s after a turn ends. This is fake-tested only; the next real run must show "transcript agrees".
