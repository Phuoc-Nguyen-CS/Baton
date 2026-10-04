# Status

_Last updated: 2026-10-04_

**Milestone:** M1: deliver one reviewable task (PLAN.md §11; architecture in `docs/architecture.md`)
**State:** M1 in progress on branch `worktree-m1` (plan approved 2026-10-04). M1.1–M1.2 done; M1.3 code done and fake-tested, real run pending.
**Next action:** once the owner approves the budget (open decision 1), run one real Haiku worker on the sandbox to verify M1.3 end to end; meanwhile M1.5 (candidate + checks) needs no real sessions.

**M0 summary:** 8 steps, 23 real sessions, findings F1–F20 in `docs/compat-record.md`; spike scripts in `spikes/` double as compatibility tests for Claude Code upgrades.

## M1 plan
Outcome: `baton task "<goal>" --check "<cmd>"` runs one worker in a Baton-made worktree, independently checks the exact result, and shows it for accept / request changes / defer, in an 80×24 TUI and as JSON. Closing the TUI or restarting the daemon loses nothing. Each slice ends with a demo command and tests; real-model runs use the sandbox repo and count against the budget.

- [x] **M1.1 Skeleton**: cargo workspace, one `baton` binary (clap: `daemon`, `task`, `status`, `decide`, `hook`, `doctor`), SQLite schema for task/attempt/session/workspace/candidate/verification/decision + audit log, `Backend` trait with a fake for tests. `baton doctor` checks `claude --version`, repo trust, git. Verify: `cargo test`, doctor on the sandbox.
- [x] **M1.2 Daemon + CLI**: daemon on a Unix socket owns state; `baton task` creates a task; `baton status --json`. Verify: state survives a daemon restart. (Intake only; dispatch, and with it the fake backend, starts in M1.3.)
- [ ] **M1.3 Claude adapter**: worktree (D1), settings file (hooks, deny rules), inline agent (D3), `claude --bg` dispatch, id parsing, `agents --json` polling; `baton hook <event>` → daemon, with local push and role guards (D4). OTel env + status line moved to M1.6 with their receiver. Verify: one real worker runs a trivial task end to end. *(Code + 40 fake-backend tests done; real run pending.)*
- [ ] **M1.4 Permissions**: `acceptEdits` + policy hook (D2); out-of-scope requests become decisions; `baton decide <id> allow|deny`; stop + resume recovery with exact-request matching. Verify: allow, deny, and late-answer cases.
- [ ] **M1.5 Candidate + checks**: on the worker's final `Stop`, snapshot the worktree as a commit on `baton/<task>`, run the check command there, store results against that tree hash; any later change invalidates them. Verify: one passing and one failing check.
- [ ] **M1.6 Usage**: OTLP receiver in the daemon (identity stripped) + status-line quota (D6), per attempt. Verify: totals match the transcript.
- [ ] **M1.7 TUI**: 80×24 and wide layouts answering "what needs me / what's progressing / what's ready to review"; review screen (goal, diff, check evidence, usage); accept / request changes / defer; native attach with terminal restore. Verify: ratatui test-backend snapshots at both sizes + a manual tmux check.
- [ ] **M1.8 Steer + recover**: "request changes" steers via hooks or stop + resume (D5); daemon start reconciles live attempts (F20). Verify: kill the daemon mid-task, restart, task continues.
- [ ] **M1.9 Demo + report**: demo script, evidence, measured usage and human decisions per task, limitations, next scope.

Accepting a task leaves the result on branch `baton/<task>`; merging into the owner's branch stays the owner's action (PLAN §6).

## Open decisions (owner)
1. **M1 real-model budget.** ★ Up to 30 Haiku sessions for M1's smoke tests (same rules as M0: trivial prompts, ask before exceeding). Blocks the real-run checks of M1.3/M1.4/M1.6/M1.8. Counter: 0 used.

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
