# Status

_Last updated: 2026-10-04_

**Milestone:** M0: prove the boundaries (PLAN.md §3, §11)
**State:** M0.1 and M0.2 done. Lifecycle results are in `docs/compat-record.md` (findings 8–11, C1–C5 Tested).
**Next action:** M0.3 per-session config spike: write `spikes/m0.3-config.sh` and run it against `~/projects/baton-sandbox`.

**Spike budget:** 7 / 20 real Claude sessions used (Haiku, trivial prompts; ask before exceeding).

## M0 plan (approved 2026-10-04)
Goal: a compatibility record plus tested answers for every Claude Code primitive M1 depends on. Spikes are throwaway shell scripts in `spikes/`, run against a disposable repo outside this one (`~/projects/baton-sandbox`, trusted by the owner; output in `spikes/out/`, gitignored). No TUI work in M0. See "Findings that change the plan" in `docs/compat-record.md`.

- [x] **M0.1 Compat record**: `docs/compat-record.md`, 15 capabilities with Doc/Probed evidence and the spike that will test each.
- [x] **M0.2 Lifecycle**: `spikes/m0.2-lifecycle.sh`. Resume only flag-free (any flag makes a copy); `--bg` ignores `--session-id`; Baton-made worktrees work and stay uncommitted; `logs` is display-only.
- [ ] **M0.3 Per-session config**: `--agents`, `--settings` (hooks + deny rules for `git push` / `gh pr`) and `--mcp-config` together with `--bg`, all set at first dispatch (they can't be changed on resume). Confirm all load and user/project settings stay in effect. To test auto-push, give the sandbox a local bare repo as `origin` and let Claude make its own worktree.
- [ ] **M0.4 Permissions**: does PermissionRequest fire in a `--bg` session? Record behavior on allow, deny, timeout and no decision. (`waitingFor: "permission prompt"` already seen.)
- [ ] **M0.5 Steering**: measure delivery, acknowledgment and latency to an idle and a busy worker via (a) `SendMessage` from another session, (b) Stop-hook `additionalContext`, (c) raw socket post (undocumented, so adapter only). Fallback: `attach`.
- [ ] **M0.6 Telemetry**: which produce data while detached: OpenTelemetry (console exporter), status line, transcript usage? Missing ≠ zero.
- [ ] **M0.7 Failure paths**: supervisor killed, cancellation, agent definition missing on resume. (Untrusted folder done in M0.2.)
- [ ] **M0.8 Write-up**: chosen architecture + known limitations in `docs/architecture.md`; owner approves → M1.

## Waiting on owner (not blocking)
- **Remote backup:** approved, but `gh` isn't installed. Either `sudo apt install gh && gh auth login`, or create an empty private repo on github.com and give me its URL.
- **Commit PLAN.md on main** if not done yet: `git add PLAN.md && git commit -m "Add PLAN.md spec"`.
- **Leftovers you may delete:** your empty background session `5dae2532` in the sandbox (`claude rm 5dae2532`), and `~/scratch/baton-sandbox` once M0.7 no longer needs an untrusted folder.

## Blockers
None.

## Done
- 2026-10-04: Repo initialized with CLAUDE.md, STATUS.md and docs/decisions.md. Confirmed Rust 1.99 and Claude Code 2.1.289 are installed.
- 2026-10-04: M0.1 compat record. Key findings: respawn ≠ resume; bg sessions auto-push and open draft PRs; cross-session messaging is a documented steering path.
- 2026-10-04: M0.2 lifecycle spike (7 sessions). Key findings: each repo needs one interactive trust; any flag on `--resume` makes a copy that can edit the original's worktree; Baton should own worktrees.
