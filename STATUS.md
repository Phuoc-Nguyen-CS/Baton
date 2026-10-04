# Status

_Last updated: 2026-10-04_

**Milestone:** M0: prove the boundaries (PLAN.md §3, §11)
**State:** M0.1 done. Compat record has documented + probed facts; nothing tested against a live session yet.
**Next action:** M0.2 lifecycle spike. Write `spikes/m0.2-lifecycle.sh`, run it against `~/scratch/baton-sandbox`, and fill C1–C5 **Tested** lines in `docs/compat-record.md`.

**Spike budget:** 0 / 20 real Claude sessions used (Haiku, trivial prompts; ask before exceeding).

## M0 plan (approved 2026-10-04)
Goal: a compatibility record plus tested answers for every Claude Code primitive M1 depends on. Spikes are throwaway shell scripts in `spikes/`, run against a disposable repo outside this one (`~/scratch/baton-sandbox`). No TUI work in M0. See "Findings that change the plan" in `docs/compat-record.md`.

- [x] **M0.1 Compat record**: `docs/compat-record.md`, 15 capabilities with Doc/Probed evidence and the spike that will test each.
- [ ] **M0.2 Lifecycle**: `--bg --name` a trivial task, preferably with a preassigned `--session-id` → match it in `agents --json` → `logs` → `stop` → resume with `claude --resume <uuid> --bg`. Record identity changes and duplicate work. Also dispatch inside a Baton-made `git worktree` and confirm no nested worktree. Don't use `respawn` as resume: it can re-run the original prompt.
- [ ] **M0.3 Per-session config**: `--agents`, `--settings` (hooks + permission deny rules for `git push` / `gh pr`) and `--mcp-config` together with `--bg`. Confirm all load and user/project settings stay in effect. Confirm the deny rules stop the documented auto-push/draft-PR on finish.
- [ ] **M0.4 Permissions**: does PermissionRequest fire in a `--bg` session? Record behavior on allow, deny, timeout and no decision, and the `waitingFor` value while blocked.
- [ ] **M0.5 Steering**: measure delivery, acknowledgment and latency to an idle and a busy worker via (a) `SendMessage` from another session, (b) Stop-hook `additionalContext`, (c) raw socket post (undocumented, so adapter only). Fallback: `attach`.
- [ ] **M0.6 Telemetry**: which produce data while detached: OpenTelemetry (console exporter), status line, transcript usage? Missing ≠ zero.
- [ ] **M0.7 Failure paths**: untrusted folder, supervisor killed, cancellation, agent definition missing on resume.
- [ ] **M0.8 Write-up**: chosen architecture + known limitations in `docs/architecture.md`; owner approves → M1.

## Waiting on owner (not blocking)
- **Remote backup:** approved, but `gh` isn't installed. Either `sudo apt install gh && gh auth login`, or create an empty private repo on github.com and give me its URL.
- **Commit PLAN.md on main** if not done yet: `git add PLAN.md && git commit -m "Add PLAN.md spec"`.

## Blockers
None.

## Done
- 2026-10-04: Repo initialized with CLAUDE.md, STATUS.md and docs/decisions.md. Confirmed Rust 1.99 and Claude Code 2.1.289 are installed.
- 2026-10-04: M0.1 compat record. Key findings: respawn ≠ resume; bg sessions auto-push and open draft PRs; worktree path not machine-readable; cross-session messaging is a documented steering path.
