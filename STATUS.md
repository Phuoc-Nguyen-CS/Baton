# Status

_Last updated: 2026-10-04_

**Milestone:** M0: prove the boundaries (PLAN.md §3, §11)
**State:** M0 plan drafted, **awaiting owner approval**
**Next action:** Owner reviews the M0 plan and answers the open decisions below; then start M0.1.

## M0 plan (draft)
Goal: a compatibility record plus tested answers for every Claude Code primitive M1 depends on. Spikes are throwaway shell scripts in `spikes/`, run against a disposable repo outside this one (`~/scratch/baton-sandbox`). No TUI work in M0.

- [ ] **M0.1 Compat record**: create `docs/compat-record.md`: per capability, list version, OS, launch mode, doc URL+date, expected, observed, fallback.
- [ ] **M0.2 Lifecycle**: `claude --bg` a named trivial task → find its session id and worktree via `claude agents --json` → `logs` → `stop` → `respawn`/resume. Record whether resume reuses or copies the id and whether work is duplicated.
- [ ] **M0.3 Per-session config**: launch with `--agents`, `--settings` (hooks) and an MCP server together. Confirm all three load and the user/project settings stay in effect.
- [ ] **M0.4 Permissions**: in a detached session, does a PermissionRequest hook fire? Record behavior on deny, timeout and no decision, plus which prompts it doesn't cover.
- [ ] **M0.5 Steering**: deliver an instruction to a live bg session with an acknowledgment and measured latency. Candidates: cross-session messaging, Stop-hook delivery, channels. Fallback: native `attach`.
- [ ] **M0.6 Telemetry**: which usage/quota signals are readable while detached (transcript usage fields, status-line fields)? Missing ≠ zero.
- [ ] **M0.7 Failure paths**: workspace trust prompt, missing daemon/TUI, cancellation, role/plugin missing on resume.
- [ ] **M0.8 Write-up**: chosen architecture + known limitations in `docs/architecture.md`; owner approves → M1.

## Open decisions (owner)
1. **Spike spend.** M0.2–M0.7 spawn real Claude sessions. ★ Small model, trivial prompts, cap ~20 sessions total, ask before exceeding.
2. **Remote backup.** No remote yet, so a lost disk loses everything. ★ Private GitHub repo after the first commit. Not blocking.

## Blockers
None.

## Done
- 2026-10-04: Repo initialized with CLAUDE.md, STATUS.md and docs/decisions.md. Confirmed Rust 1.99 and Claude Code 2.1.289 are installed.
