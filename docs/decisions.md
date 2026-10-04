# Decisions

Settled unless the owner reopens them. Newest last. (R) = reversible, decided by Claude.

| Date | Decision | By | Notes |
|---|---|---|---|
| 2026-10-04 | Build on Claude Code native background sessions (`--bg`, `agents --json`, `attach`, `logs`, `stop`, `respawn`), not a custom tmux/PTY backend | Owner | Each primitive verified in M0 |
| 2026-10-04 | Claude-only backend; keep a small seam for other vendors but implement only Claude | Owner | |
| 2026-10-04 | Rust: ratatui/crossterm, tokio, SQLite, serde, clap | Owner | |
| 2026-10-04 | Supervised: owner approves plan + policy together; agents advise, owner decides | Owner | PLAN.md §5 |
| 2026-10-04 | Personal tool first, maybe open source later | Owner | |
| 2026-10-04 | Progress lives in the repo: CLAUDE.md imports STATUS.md and this file; commit after each chunk | Claude (R) | |
| 2026-10-04 | M0 spikes are throwaway shell scripts in `spikes/`; Rust code starts at M1 | Claude (R) | |
| 2026-10-04 | M0 plan approved | Owner | STATUS.md |
| 2026-10-04 | Spike spend: Haiku, trivial prompts, ≤20 real sessions for all of M0; ask before exceeding | Owner | Counter in STATUS.md |
| 2026-10-04 | Remote backup: private GitHub repo | Owner | Owner creates it; not blocking |
| 2026-10-04 | Spike sandbox is `~/projects/baton-sandbox` (was `~/scratch/…`); `~/scratch/baton-sandbox` stays untrusted for trust tests | Claude (R) | Owner trusts the sandbox once |
