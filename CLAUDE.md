# Baton

Local Rust TUI + daemon that coordinates Claude Code background sessions and delivers verified, reviewable changes. "Baton" is a working name.

## Source of truth
- **Spec:** `PLAN.md` Part B (the build prompt). Part A is the rationale. Read the section for the current milestone; don't load the whole file unless needed.
- **Where we are / what's next:** `STATUS.md` (imported below).
- **Settled decisions:** `docs/decisions.md` (imported below). Don't reopen these unless the owner does.
- **History:** `git log`.

@STATUS.md
@docs/decisions.md

## Session rules
- **Start:** the imported STATUS.md is the handoff. Continue from "Next action". If it disagrees with `git log` or the working tree, reconcile STATUS.md first.
- **While working:** one milestone at a time (PLAN.md §11). Label claims as *documented*, *assumption*, or *tested (with version)*. Never fabricate validation.
- **End of every chunk of work** (and before the chat may be cleared): update STATUS.md (checkboxes, Next action, open decisions), add new decisions to docs/decisions.md, then commit. A chunk isn't done until STATUS.md reflects it.
- Keep STATUS.md under ~80 lines. Finished detail goes into commit messages or `docs/`, not STATUS.md.

## Owner decides
The owner makes consequential calls (scope, spend, permissions, publication, architecture). Present those as a decision card: 2–4 options, tradeoffs, ★ recommendation, reversible or not. Decide small reversible things yourself and record them in docs/decisions.md.

## Environment
- WSL2 Linux, tmux 3.6, git 2.53, Claude Code 2.1.289 (re-check `claude --version`; record changes in the compat record).
- Rust 1.99 is in `~/.cargo/bin` but not on PATH in non-interactive shells: prefix commands with `. ~/.cargo/env &&`.
- Not installed: `gh`, `sqlite3` CLI.
- Spikes that spawn real Claude sessions spend subscription quota: get owner OK first unless docs/decisions.md records a spend policy.
