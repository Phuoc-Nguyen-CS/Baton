# M1 smoke runs

Real Claude Code sessions run by Baton during M1 (budget: ≤30 Haiku sessions, docs/decisions.md). Claude Code 2.1.289, WSL2, sandbox `~/projects/baton-sandbox`, state dir `~/.local/state/baton-smoke`. Evidence was collected with the job's helper scripts (status JSON, `last-message.txt`, check logs, the audit table, `claude agents --json --all`, git).

| # | Date | Slice | Task | Sessions | Result |
|---|---|---|---|---|---|
| 1 | 2026-10-04 | M1.3 + M1.5 | 1: "Create greeting.txt … Hello from Baton. Use only the Write tool." `--check "grep -qx 'Hello from Baton' greeting.txt"` `--model haiku` | 1 | ✅ `review_ready`, 1 of 1 checks passed |

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
