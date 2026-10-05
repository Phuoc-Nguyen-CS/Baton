# Status

_Last updated: 2026-10-04_

**Milestone:** M1: deliver one reviewable task (PLAN.md §11; architecture in `docs/architecture.md`)
**State:** M1 in progress on branch `worktree-m1` (plan approved 2026-10-04). M1.1–M1.6 done; real runs 1–3 passed (`docs/m1-smoke.md`).
**Next action:** finish M1.7: the manual tmux check (a private `tmux -L` server, 80×24 then 160×40, attach, quit, terminal restored). The scripted attempt failed to start tmux inside the build session's sandbox, so run it by hand. Then M1.8 (request changes + daemon-restart reconcile) and M1.9. Note: the account's 5 h quota read 85% at 17:47 (mostly this build session).

**M0 summary:** 8 steps, 23 real sessions, findings F1–F20 in `docs/compat-record.md`; spike scripts in `spikes/` double as compatibility tests for Claude Code upgrades.

## M1 plan
Outcome: `baton task "<goal>" --check "<cmd>"` runs one worker in a Baton-made worktree, independently checks the exact result, and shows it for accept / request changes / defer, in an 80×24 TUI and as JSON. Closing the TUI or restarting the daemon loses nothing. Each slice ends with a demo command and tests; real-model runs use the sandbox repo and count against the budget.

- [x] **M1.1 Skeleton**: cargo workspace, one `baton` binary (clap: `daemon`, `task`, `status`, `decide`, `hook`, `doctor`), SQLite schema for task/attempt/session/workspace/candidate/verification/decision + audit log, `Backend` trait with a fake for tests. `baton doctor` checks `claude --version`, repo trust, git. Verify: `cargo test`, doctor on the sandbox.
- [x] **M1.2 Daemon + CLI**: daemon on a Unix socket owns state; `baton task` creates a task; `baton status --json`. Verify: state survives a daemon restart. (Intake only; dispatch, and with it the fake backend, starts in M1.3.)
- [x] **M1.3 Claude adapter**: worktree (D1), settings file (hooks, deny rules), inline agent (D3), `claude --bg` dispatch, id parsing, `agents --json` polling; `baton hook <event>` → daemon, with local push and role guards (D4). OTel env + status line moved to M1.6 with their receiver. Verify: one real worker runs a trivial task end to end. (Real run 1: `review_ready` in 9.6 s.)
- [x] **M1.4 Permissions**: `acceptEdits` + policy hook (D2); out-of-scope requests become decisions; `baton decide <id> allow|deny`; stop + resume recovery with exact-request matching. Verify: allow, deny, and late-answer cases. (Real run 2: all three delivered.)
- [x] **M1.5 Candidate + checks**: on the worker's final `Stop`, snapshot the worktree as a commit on `baton/<task>`, run the check command there, store results against that tree hash; any later change invalidates them. Verify: one passing and one failing check. (Fake-backend demo: "1 of 2 checks passed".)
- [x] **M1.6 Usage**: OTLP receiver in the daemon (identity stripped) + status-line quota (D6), per attempt. Verify: totals match the transcript. (Real run 3: exact match; the re-read fix still needs a real "transcript agrees".)
- [ ] **M1.7 TUI**: 80×24 and wide layouts answering "what needs me / what's progressing / what's ready to review"; review screen (goal, diff, check evidence, usage); accept / request changes / defer; native attach with terminal restore. Verify: ratatui test-backend snapshots at both sizes + a manual tmux check. *(Code + 10 TUI tests incl. golden snapshots at 80×24 and 160×40 done; `baton review accept|defer` CLI; manual tmux check pending.)*
- [ ] **M1.8 Steer + recover**: "request changes" steers via hooks or stop + resume (D5); daemon start reconciles live attempts (F20). Verify: kill the daemon mid-task, restart, task continues.
- [ ] **M1.9 Demo + report**: demo script, evidence, measured usage and human decisions per task, limitations, next scope.

Accepting a task leaves the result on branch `baton/<task>`; merging into the owner's branch stays the owner's action (PLAN §6).

## Open decisions (owner)
None. (M1 budget approved 2026-10-04: ≤30 Haiku sessions. Counter: 3 used; log in `docs/m1-smoke.md`.)

## Waiting on owner (not blocking)
- **Remote backup:** approved, but `gh` isn't installed. Either `sudo apt install gh && gh auth login`, or create an empty private repo on github.com and give me its URL.
- **Commit PLAN.md on main** if not done yet: `git add PLAN.md && git commit -m "Add PLAN.md spec"`.
- **Leftovers you may delete:** `~/scratch/baton-sandbox` (M0 no longer needs an untrusted folder). Keep `~/projects/baton-sandbox*` for M1 smoke tests.

## Blockers
None.

## Done
- 2026-10-04: Repo initialized with CLAUDE.md, STATUS.md and docs/decisions.md. Confirmed Rust 1.99 and Claude Code 2.1.289 are installed.
- 2026-10-04: M0.1 compat record. Key findings: respawn ≠ resume; bg sessions auto-push and open draft PRs; cross-session messaging is a documented steering path.
- 2026-10-04: M0.2 lifecycle spike (7 sessions). Key findings: each repo needs one interactive trust; any flag on `--resume` makes a copy that can edit the original's worktree; Baton should own worktrees.
- 2026-10-04: M0.3 per-session config spike (6 sessions). Key findings: everything loads together and survives resume; `--agents` inline only; agents need `ToolSearch` for MCP; deny rule blocks push; verify outcomes, don't trust worker replies.
- 2026-10-04: M0.4 permissions spike (2 sessions). Key findings: Baton can answer permission requests via a bounded hook wait; timeouts fall back to the native prompt; stuck prompts recover via stop + resume.
- 2026-10-04: M0.5 steering spike (3 sessions, including the resume test that made a copy). Key findings: Baton can steer with its own hooks + CLI; `SendMessage` is near-instant but needs a Claude sender; `state` is unreliable for "finished".
- 2026-10-04: M0.6 telemetry spike (2 sessions). Key findings: all usage sources agree while detached; quota only via the status line; mind delta vs cumulative counters and strip identity from OTel.
- 2026-10-04: M0.7 failure paths (3 sessions). Key findings: a deleted project agent silently widens a resumed worker to default tools, visible only via `SessionStart` `agent_type`; workers outlive a stopped or killed supervisor and are re-adopted.
- 2026-10-04: M0.8 architecture approved by the owner (all ★); M0 closed.
- 2026-10-04: M1.1 skeleton: `crates/baton` (clap CLI, SQLite schema v1 + audit log, `Backend` trait + fake, `baton doctor`); 13 tests; doctor passes on the sandbox and flags the untrusted `~/scratch` repo.
- 2026-10-04: M1.2 daemon + CLI: `baton daemon` (Unix socket, single-instance lock), idempotent `baton task`, `baton status [--json]`; 20 tests incl. SIGTERM and SIGKILL restarts of the real binary; demoed on the sandbox.
- 2026-10-04: M1.3 dispatch lifecycle, Claude adapter, `baton hook` guards; real run 1 (1 session): dispatch → hooks (role ok) → `Stop` → candidate → check passed → `review_ready`.
- 2026-10-04: M1.5 candidate + checks: `Stop` → snapshot commit on `baton/<task>` → checks in the worktree → `review_ready` with counts; drift invalidates; timeouts kill the process group; 49 tests.
- 2026-10-04: M1.4 permissions: policy + durable decisions + `baton decide`; late answers delivered by stop + flag-free resume and applied to the exact request once; real run 2 (1 session) passed allow, deny and late. Fixed: idle workers held the worker slot; a guard-in-match deadlock.
- 2026-10-04: M1.6 usage: OTLP receiver + `api_request` rows, status-line quota, transcript cross-check; real run 3 (1 session): OTel totals equal the transcript exactly, no identity stored. Fixed: transcript read before its last entry was written.
