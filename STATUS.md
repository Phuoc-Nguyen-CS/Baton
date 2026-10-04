# Status

_Last updated: 2026-10-04_

**Milestone:** M0: prove the boundaries (PLAN.md §3, §11)
**State:** M0.1–M0.7 done; M0.8 drafted: `docs/architecture.md`, **awaiting owner approval**.
**Next action:** owner reviews `docs/architecture.md` §6 and answers decision cards D1–D7. Record the answers in `docs/decisions.md`, mark M0 done, then plan M1's first slice.

**Spike budget:** 23 / 30 real Claude sessions used (Haiku, trivial prompts; ask before exceeding).

## M0 plan (approved 2026-10-04)
Goal: a compatibility record plus tested answers for every Claude Code primitive M1 depends on. Spikes are throwaway shell scripts in `spikes/`, run against a disposable repo outside this one (`~/projects/baton-sandbox`, trusted by the owner; output in `spikes/out/`, gitignored). No TUI work in M0. See "Findings that change the plan" in `docs/compat-record.md`.

- [x] **M0.1 Compat record**: `docs/compat-record.md`, 15 capabilities with Doc/Probed evidence and the spike that will test each.
- [x] **M0.2 Lifecycle**: `spikes/m0.2-lifecycle.sh`. Resume only flag-free (any flag makes a copy); `--bg` ignores `--session-id`; Baton-made worktrees work and stay uncommitted; `logs` is display-only.
- [x] **M0.3 Per-session config**: `spikes/m0.3-config.sh`. Agent (inline only) + hooks + MCP + user/project instructions all load on one `--bg` session and come back on flag-free resume; file-based config is re-read on resume; a custom agent needs `ToolSearch` for MCP tools; a `git push` deny rule holds; workers don't report their own failures.
- [x] **M0.4 Permissions**: `spikes/m0.4-permissions.sh`. A `PermissionRequest` hook waiting on a Baton inbox gets allow/deny honoured; past its timeout it's killed and the native prompt stays (late answers do nothing); stop + flag-free resume re-asks through the hook; a `PreToolUse` deny hook blocks and records pushes.
- [x] **M0.5 Steering**: `spikes/m0.5-steering.sh`. Busy: `PostToolUse`/`Stop` hooks deliver Baton's queued instruction at the next tool boundary or turn end. Idle: `claude stop` + flag-free resume with the instruction (~2 s). `SendMessage` works for both but needs a Claude sender. Resuming a live idle session makes a copy.
- [x] **M0.6 Telemetry**: `spikes/m0.6-telemetry.sh`. OTel, transcript and status line agree exactly on tokens/cost while detached; quota only from the status line (refreshes while idle); OTel = per-process deltas, status-line cost = cumulative snapshot; OTel events carry owner identity.
- [x] **M0.7 Failure paths**: `spikes/m0.7-failures.sh`. A deleted project agent makes a resumed worker silently widen to default tools (detect via `SessionStart` `agent_type`). Workers survive a supervisor stop or `kill -9` and get re-adopted by the next one. Untrusted folder done in M0.2; mid-tool cancellation in M0.4.
- [ ] **M0.8 Write-up**: `docs/architecture.md` drafted (shape, worker lifecycle, usage, state, 10 limitations, decision cards D1–D7). Owner approves → M1.

## Waiting on owner (not blocking)
- **Remote backup:** approved, but `gh` isn't installed. Either `sudo apt install gh && gh auth login`, or create an empty private repo on github.com and give me its URL.
- **Commit PLAN.md on main** if not done yet: `git add PLAN.md && git commit -m "Add PLAN.md spec"`.
- **Leftovers you may delete:** your empty background session `5dae2532` in the sandbox (`claude rm 5dae2532`), and `~/scratch/baton-sandbox` once M0.7 no longer needs an untrusted folder.

## Blockers
None. (Supervisor test: owner chose to run it from the build session; see Next action.)

## Done
- 2026-10-04: Repo initialized with CLAUDE.md, STATUS.md and docs/decisions.md. Confirmed Rust 1.99 and Claude Code 2.1.289 are installed.
- 2026-10-04: M0.1 compat record. Key findings: respawn ≠ resume; bg sessions auto-push and open draft PRs; cross-session messaging is a documented steering path.
- 2026-10-04: M0.2 lifecycle spike (7 sessions). Key findings: each repo needs one interactive trust; any flag on `--resume` makes a copy that can edit the original's worktree; Baton should own worktrees.
- 2026-10-04: M0.3 per-session config spike (6 sessions). Key findings: everything loads together and survives resume; `--agents` inline only; agents need `ToolSearch` for MCP; deny rule blocks push; verify outcomes, don't trust worker replies.
- 2026-10-04: M0.4 permissions spike (2 sessions). Key findings: Baton can answer permission requests via a bounded hook wait; timeouts fall back to the native prompt; stuck prompts recover via stop + resume.
- 2026-10-04: M0.5 steering spike (3 sessions, including the resume test that made a copy). Key findings: Baton can steer with its own hooks + CLI; `SendMessage` is near-instant but needs a Claude sender; `state` is unreliable for "finished".
- 2026-10-04: M0.6 telemetry spike (2 sessions). Key findings: all usage sources agree while detached; quota only via the status line; mind delta vs cumulative counters and strip identity from OTel.
- 2026-10-04: M0.7 failure paths, part 1 (2 sessions). Key finding: a deleted project agent silently widens a resumed worker to default tools, with no warning; only `SessionStart` `agent_type` shows it.
- 2026-10-04: M0.7 supervisor test (1 session, run detached from the build session). Workers outlive a stopped or killed supervisor and are re-adopted; all live sessions survived.
- 2026-10-04: M0.8 architecture drafted (`docs/architecture.md`), awaiting owner decisions D1–D7.
