# M1 smoke runs

Real Claude Code sessions run by Baton during M1 (budget: ≤30 Haiku sessions, docs/decisions.md). Claude Code 2.1.289, WSL2, sandbox `~/projects/baton-sandbox`, state dir `~/.local/state/baton-smoke`. Evidence was collected with the job's helper scripts (status JSON, `last-message.txt`, check logs, the audit table, `claude agents --json --all`, git).

| # | Date | Slice | Task | Sessions | Result |
|---|---|---|---|---|---|
| 1 | 2026-10-04 | M1.3 + M1.5 | 1: "Create greeting.txt … Hello from Baton. Use only the Write tool." `--check "grep -qx 'Hello from Baton' greeting.txt"` `--model haiku` | 1 | ✅ `review_ready`, 1 of 1 checks passed |
| 2 | 2026-10-04 | M1.4 | 2: "Run these three Bash commands one at a time … print(101) … print(102) … print(103). If a command is denied, don't retry it" `--check "test -f README.md"` `--model haiku` | 1 | ✅ allow, deny and late allow all delivered; `review_ready` |
| 3 | 2026-10-04 | M1.6 | 3: "Create a file named usage.txt containing the single word ok. Use only the Write tool." `--check "grep -qx ok usage.txt"` `--model haiku` | 1 | ✅ OTel totals = transcript; quota seen; no identity stored. Cross-check read too early (fixed) |
| 4 | 2026-10-04 | M1.8 | 4: "Create a file named hello.txt containing the single word hello. Use only the Write tool." `--check "grep -qx hello hello.txt"` `--model haiku` | 1 | ✅ daemon SIGKILLed mid-turn, worker finished unheard, restart reconciled it to `review_ready`; request changes reached the same session; accepted |
| 5 | 2026-10-04 | (unlogged) | 5: "Create bye.txt containing the single word bye. Use only the Write tool." `--check "grep -qx bye bye.txt"` `--model haiku` | 1 | ✅ `review_ready` in 10 s. Logged on 2026-10-06 from `baton.db`; who ran it isn't recorded. Its usage revealed F21 |
| 6 | 2026-10-06 | M1.9 | 6: `demo/m1-demo.sh --idle 200` (notes.txt + `python3 -c 'print(6*7)'`), 2 checks, `--model haiku`, Claude Code **2.1.292** | 1 | ✅ permission allow → review → live attach and back → idle → request changes → accept; transcript agrees; 1 side request |

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

## Manual TUI check (M1.7, fake backend, no sessions spent)

Run on 2026-10-04: a private tmux server (`tmux -L baton-m17`, tmux 3.6) with a fake-backend daemon. Task 1 was at review (1 of 2 checks passed); task 2 was running.

**Tested:**
- At 80×24 the list fits, and both the task screen (goal, worker, usage, candidate checks, change stat, handoff) and the diff screen render. The handoff falls below the fold and needs `j` to scroll into view.
- Resizing the live window to 160×40 switched to the split list + detail layout with no restart. Resizing back to 80×24 restored the narrow layout.
- `t` (attach) left the alternate screen, ran `claude attach fake0001`, and came back to the TUI with "claude attach fake0001 exited with exit status: 1". The session doesn't exist, so only the failure path was exercised; attaching to a live worker is part of the M1.9 demo.
- `q` restored the terminal: tmux reported `alternate_on=0`, and the shell echoed normally.

**Limitation:** when attach fails, claude's own message ("No job matching …") goes to the normal screen. It only shows up after you quit Baton; inside the TUI you see just the exit status.

## Run 4: daemon restart + request changes (M1.8)

**Tested (2.1.289):**
- **Kill mid-turn:** Baton dispatched session `6ecb991c` 1.2 s after intake. Its daemon got `kill -9` right away, while `claude agents` showed the worker `working`. With no daemon running, the worker finished: 30 s later its row read `done`/`idle`. Its `Stop` hook had nowhere to go.
- **Restart:** the new daemon reconciled before its first tick. The attempt was `running` and the worker `idle`, so the turn had ended unheard. Baton moved the task to verifying ("worker finished without a handoff; running checks (found after a daemon restart)"), snapshotted candidate `30143e3`, ran the check (exit 0) and reached `review_ready` about 1.7 s after starting.
- **Request changes:** `baton review 4 changes --candidate 30143e3 --note "Also add a second line … world …"`. Baton stopped the idle worker, waited for its process to go, and resumed it flag-free with the note. That took 2.7 s, kept the same session id `6ecb991c` (no copy), and the `SessionStart` role stayed intact. The worker edited `hello.txt` and ended with a new handoff. Baton snapshotted candidate `c4f30bc` (`hello\nworld`), re-ran the check and was back at `review_ready` 15 s after the request.
- **Accept:** `baton review 4 accept --candidate c4f30bc` left both candidates on `baton/4` and stopped the idle worker. The sandbox's main checkout stayed clean.

**Limitations seen:**
- **No handoff after the kill.** The worker's `SessionStart` also fired while the daemon was down, so at reconcile time Baton didn't know the transcript path, and `last-message.txt` stayed empty. The transcript did hold the full handoff. The path arrived seconds later with a status-line update, too late for reconcile. Verification doesn't depend on the handoff, but a `STATUS: blocked` worker caught this way would go to review instead of waiting for input.
- **Usage gap.** OTel events sent while the daemon was down are lost, so the first turn's usage is missing and `status` says "transcript differs" (4 requests recorded). That's correct behaviour, but the M1.6 "transcript agrees" confirmation is still open.

**Found and fixed:** during the restart the task read "running | worker session ended (other)", because the stop's `SessionEnd` hook overwrote the reason. Once the worker has the changes, the reason now says "the worker has the owner's changes".

## Run 5: an unlogged run, and the idle recap request (F21)

Found on 2026-10-06 in the smoke `baton.db`: task 5 (session `9dc5dc37`, 2026-10-04 20:54) reached `review_ready` 10 s after intake, 1 of 1 checks passed. No notes record who ran it; it counts against the M1 budget.

**Tested (2.1.289):** OTel recorded 4 requests and the transcript 3. The 3 the transcript lists match OTel exactly, request by request (26 / 470 / 19,332 / 3,038 in total), which is the real-run "transcript agrees" that M1.6 left open. The 4th (`req_011CfiRE2v…`, 79 in / 386 out) came 196 s after the turn ended. It is Claude Code's `away_summary` recap of the idle session: the transcript has a `system`/`away_summary` entry with no `usage`.

**Found and fixed:** every task left at review for more than ~3 min would have shown "transcript differs". Transcripts now keep their `requestId`s, and OTel requests no transcript lists are shown as side requests outside the comparison ("transcript agrees; 1 side request").

## Run 6: the M1.9 demo (`demo/m1-demo.sh --idle 200`)

Claude Code **2.1.292** (`baton doctor` warned it is untested). Captured screens and logs: `target/m1-demo/run6/` in the M1.9 worktree (not committed).

**Tested (2.1.292)**, times from intake (audit log):
- 0.86 s dispatched (session `22679755`). 5.3 s: `python3 -c 'print(6*7)'` became decision 6; the script's `baton decide 6 allow` was applied at 6.2 s and the command ran (output `42`).
- 10.3 s turn ended; 11.95 s `review_ready`, candidate `0ffdae5`: 1 of 2 checks passed. The failure was the demo's own check: `test $(wc -l < notes.txt) -ge 1` counts newlines, and the worker wrote `one` without one. Baton reported it as it should; the owner decides.
- TUI at 80×24: list, then the review screen (goal, worker, usage with "transcript agrees", both check results, diff stat). `t` attached to the **live** worker: Claude's screen with the handoff and Baton's status line (`baton · attempt 6 · baton-worker`). `Ctrl+Z` returned to the review screen, which said "back from worker 22679755"; `q` quit cleanly.
- Idle at review: the `away_summary` request arrived 183 s after the turn. `status` then read "3 requests … (transcript agrees; 1 side request)", which confirms the run-5 fix on real data.
- 225.4 s request changes ("Add a second line to notes.txt: two."): the worker had them 2.4 s later (same session). 236.0 s new candidate `40f38ea` (`one\ntwo`), 2 of 2 checks passed; accepted. Both candidates are on `baton/6`; the sandbox's main checkout was untouched.
- Final usage: 6 requests, 123 in / 1,388 out / 37,784 cache read / 8,216 cache write, $0.0273 est. (transcript agrees; 1 side request). Human decisions: permission allow, request changes, accept.

**Limitation seen:** right after the allow, the task flipped back to `waiting_permission` for 2 s (6.29 s → 8.29 s) with no new decision. The cause is likely a stale `waitingFor: permission prompt` in Claude's listing, or a late `permission_prompt` notification; the log doesn't say which. It's cosmetic: nothing waited on it.
