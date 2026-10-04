# Compatibility record

What Baton relies on from Claude Code, and how we know it. Update the **Tested** lines as M0 spikes run; never fill them from docs.

Evidence labels:
- **Doc**: official docs, fetched 2026-10-04 (pages show no per-page date; the agent-view changelog runs to v2.1.288).
- **Probed**: seen locally in `--help` or read-only output on the reference setup below. No model calls.
- **Tested**: observed in a spike. Cite the spike script.
- **Assumption**: not yet backed by either.

## Reference setup

| Item | Value |
|---|---|
| Claude Code | 2.1.289 |
| OS | Linux 6.18.33.2-microsoft-standard-WSL2 |
| tmux | 3.6 |
| git | 2.53.0 |
| Rust | 1.99.0 (`~/.cargo/bin`) |
| Auth | claude.ai subscription (OAuth), owned by the CLI |
| Background supervisor | on-demand "transient" daemon; `claude daemon status` reports pid, version, socket dir `/tmp/cc-daemon-<uid>/…` (Probed) |

Doc pages: [agent-view], [cli-reference], [hooks], [cross-session-messaging], [channels], [channels-reference], [statusline], [monitoring-usage], [worktrees], [headless], [legal-and-compliance].

## Findings that change the plan

1. **`respawn` is not resume.** `respawn` restarts a session's process (for a new binary). If no transcript is on disk it **re-runs the original prompt**, which can duplicate work. Resume is `claude attach <id>` or `claude --resume <uuid> --bg` **with no other flags** (see 9). (Doc, Probed, Tested)
2. **Background sessions publish on their own.** Changelog v2.1.198: a session isolated in a worktree "commits, pushes its own isolated branch … and opens a draft pull request when it finishes instead of asking first." This conflicts with Baton's policy (no push without the owner). Fallback: deny `git push` and `gh pr` via `--settings` permission rules, and verify. (Doc)
3. **Baton should create the worktree and dispatch inside it.** Claude's own worktree is created lazily, shows up in `agents --json` `cwd` only while the process lives, and comes with a self-made commit. A session dispatched inside a Baton-made worktree stays there, nests nothing, leaves its edits uncommitted for Baton to snapshot, and `claude rm` never deletes that worktree. (Tested)
4. **Steering has a documented native path.** Cross-session messaging works for background sessions: an idle session starts a new turn, a busy one reads it between tool calls. The socket's auth line is documented but **the message line format is not**, so posting raw to the socket stays an Assumption. (Doc)
5. **`--bare` is unusable for workers.** It skips hooks, CLAUDE.md and the messaging inbox. (Doc, Probed)
6. **A missing role widens a resumed session silently.** If a session's agent definition is gone on resume, the session "continues with the default tools" plus a transcript warning. Baton must check before resuming. (Doc)
7. **Telemetry while detached is unproven.** The status line has quota fields, but it only runs when the UI renders. OpenTelemetry export is documented and doesn't need a UI, which makes it the best candidate. (Doc; Assumption for detached)
8. **Every target repo must be trusted interactively once.** A script can't accept the trust dialog, and trusting a parent folder didn't cover a new repo inside it. Linked worktrees of a trusted repo pass, wherever they live. Baton needs a one-time "trust this repo" step for the owner. (Tested)
9. **Any flag on `--resume` makes a copy.** `--resume <uuid> --bg` plus *any* other flag (`--model`, `--permission-mode`…) starts a copy with a new id: "keeps its own saved options, so the flags you passed started a copy". With no flags it wakes the same session with its saved options. Resuming a running session also copies. (Tested)
10. **Copies are dangerous.** A copy carries the full conversation, starts in the original's dispatch directory, and edited the **original's** worktree. Baton must never create copies by accident: resume flag-free, and only after confirming the session isn't running. (Tested)
11. **`--bg` ignores `--session-id`.** It prints a warning and assigns its own id; the short id is the first 8 characters of the session UUID. Baton reads identity from the dispatch output. (Tested)

## Capabilities

### C1 Spawn
- **Doc:** `claude --bg [--name N] "<prompt>"` returns immediately and prints the short id. `--bg` with `-p` is rejected (v2.1.198). Background sessions inherit `PATH` and settings from the dispatching shell.
- **Probed:** `--bg`, `-n/--name`, `--session-id <uuid>` exist in help.
- **Tested (2026-10-04, `spikes/m0.2-lifecycle.sh`):** returns in 0.66–0.85 s (n=7). Prints `backgrounded · <id> · <name>` plus hint lines, with ANSI colour even when stdout isn't a TTY. `--session-id` is ignored with `warning: --bg manages the session id; ignoring --session-id`. The row appears in `agents --json` immediately as `working`, before `pid`/`status` exist. Workers ran with the owner's normal settings (a `PostToolUse` hook message appeared; source not identified).
- **Fallback:** strip ANSI, parse the id from the `backgrounded · <id>` line (a `note:` line can name another session first), then match it in `agents --json`.

### C2 List and state
- **Doc:** `claude agents --json [--all] [--cwd <path>]`. `state`: `working` · `blocked` · `done` · `failed` · `stopped`. `status` (process alive): `busy` · `waiting` · `idle`. `waitingFor`: `permission prompt` · `input needed` · `sandbox request` · `worker request` · `dialog open`.
- **Probed:** fields `id` (8 chars), `sessionId` (UUID), `name`, `kind` (`background`), `pid`, `cwd`, `startedAt` (epoch ms), `state`, `status`. Seen: `working`, `blocked`, `done`; `status` missing on finished sessions. Not yet seen: `waitingFor`, `failed`, `stopped`.
- **Tested:** seen `working` → `done`/`idle`, and `blocked`/`waiting` with `waitingFor: "permission prompt"`. `cwd` follows the live process into its worktree. `startedAt` changes with each new process, so it isn't an identity. After `stop`, a `done` session stays `state: done` (not `stopped`), loses `pid`/`status`, and `cwd`/`startedAt` revert to dispatch-time values. Auto-naming can rename a session (a copy became "file append operation"). An empty interactive-then-backgrounded session showed `blocked`/`idle` with no `waitingFor`.
- **Fallback:** poll the CLI and treat unknown values as `unknown`. Never read `~/.claude/jobs/*/state.json` (internal).

### C3 Identities
- **Doc:** short id is for `attach`/`logs`/`stop`/`respawn`/`rm`; UUID is for `--resume`. `--resume <uuid> --bg` continues under the same id, or starts a copy and prints a `note:` line when the session is already running. `--resume <short-id>` starts a copy. `--fork-session` always copies.
- **Tested:** short id = first 8 chars of the UUID (7 of 7 sessions seen). `--resume <uuid> --bg` with no flags: `note: woke session 022db4c9 with its saved options (--name, --permission-mode, --allowedTools, --model)`, same id, new pid, back in its worktree `cwd`. With any flag: `note: background session … keeps its own saved options, so the flags you passed started a copy as <new>`. While running: `note: session … is already running in the background, so this started a copy as <new>`. A copy carries the conversation but not the worktree `cwd`.
- **Fallback:** Baton stores backend identity per attempt, resumes flag-free only after checking the session isn't running, and treats any new id in the output as a copy to stop.

### C4 Workspace / worktree
- **Doc:** lazy worktree under `.claude/worktrees/`, skipped when already in a linked worktree, outside git, or with `worktree.bgIsolation: "none"`. A `WorktreeCreate` hook replaces git's default. Deleting a session keeps worktrees that hold unpushed commits.
- **Tested:** Claude-made: `.claude/worktrees/<adjective-noun>` on branch `worktree-<name>`, git-locked with reason `claude session <name> (pid … start …)`, and the session committed its edit itself (authored as the owner's git identity, no remote so no push seen). The main checkout then shows an untracked `.claude/`. Baton-made (`git worktree add`, both inside `.claude/worktrees/` and in a sibling directory): the session stayed in it, created no nested worktree, and left its edit **uncommitted**.
- **Fallback:** a `WorktreeCreate` hook reports the path to Baton.

### C5 Attach, logs, stop, delete
- **Doc/Probed:** `attach <id>` takes over the terminal (fullscreen); `←`/`/exit` detaches, `Ctrl+Z` drops to the shell; detaching never stops the session. Unsent input blocks detach. `logs <id>` prints recent terminal output. `stop|kill <id>` keeps the conversation. `rm <id>` deletes the session and its worktree when safe (`--discard-unpushed`, `--force-remove-worktree`).
- **Tested:** `logs` is a raw screen capture (ANSI, no newlines, partial redraws such as "Commtted"), so it's display-only after sanitizing. It does show pending permission dialogs in full. `stop` prints `stopped <id>` and `worktree retained at <path>`. `rm` works on live idle sessions. It refused a Claude-made worktree with uncommitted changes (`kept … Deleting it would lose them`) and then with unpushed commits (`kept … 2 unpushed commits … claude rm <id> --discard-unpushed <sha>@<worktree-id>`). With that flag it removed the worktree **and its branch**. It removed sessions in Baton-made worktrees and left those worktrees untouched. `attach` not tested (needs a TTY).
- **Fallback:** Baton cleans up its own worktrees; it treats `rm`'s `kept` output as "work at risk" and asks the owner.

### C6 Supervisor and machine restart
- **Doc:** a supervisor owns sessions; it stops idle processes after ~1 h unless pinned. Shutdown stops sessions: within 48 h they show `failed` and restart from where they left off; after that, `stopped`. Sleep is survived.
- **Probed:** "Service install is disabled in this version — the daemon runs on demand and exits when the last client disconnects."
- **Tested:** — (M0.7: kill the supervisor; confirm sessions and state)
- **Fallback:** Baton reconciles from `agents --json --all` on startup; unknown stays unknown.

### C7 Per-session configuration
- **Doc/Probed:** `--agents <json|file>`, `--agent`, `--settings <file|json>` (ranks below managed settings, above user/project/local files), `--setting-sources`, `--mcp-config`, `--strict-mcp-config`, `--plugin-dir`, `--add-dir`, `--append-system-prompt`, `--permission-mode`, `--model`, `--effort`, `--tools`, `--restricted`. `--restricted` drops code-running tools and user/project/local settings, so it doesn't fit workers that run checks.
- **Tested:** — (M0.3: all of these at once with `--bg`, user/project settings still in effect)
- **Fallback:** a project-scoped `.claude/agents/` file in the Baton-made worktree.

### C8 Role restore on resume
- **Doc:** resume restores the agent and its tool restrictions; a missing agent means default tools plus a warning.
- **Tested:** — (M0.7)
- **Fallback:** Baton verifies the agent definition exists before resume and refuses to resume otherwise.

### C9 Permission routing
- **Doc:** `PermissionRequest` fires when Claude is about to prompt, in default mode. A hook can allow or deny (`hookSpecificOutput.decision`); exit code 2 isn't honored. With no decision, a background session shows the prompt and goes `blocked` / `waitingFor: permission prompt`; in sessions that can't prompt, the call is denied. Doesn't fire for sandbox network requests (use Notification `permission_prompt`, which fires after ~6 s). Default command-hook timeout: 600 s. `defer` works only with `-p`. A reply typed in agent view does **not** answer a permission dialog.
- **Tested (M0.2, incidental):** with `--permission-mode acceptEdits` and `--allowedTools "Bash(git add:*) Bash(git commit:*) …"`, a worker still blocked on `cd <worktree> && git add … && git commit …` with "This command changes directory before running a version-control command, which can pick up untrusted hooks or repository configuration from the target directory." The row showed `blocked`/`waiting`/`waitingFor: "permission prompt"` and `logs` showed the dialog. M0.4 still to test hooks.
- **Fallback:** keep the prompt native; Baton shows "attach to answer".

### C10 Steering a live session
- **Doc, cross-session messaging (v2.1.224+):** `ListAgents`/`SendMessage` between local sessions, including background ones, over a per-session Unix socket (`CLAUDE_CODE_MESSAGING_SOCKET`). Delivery: idle → new turn; busy → read between tool calls. Inbound controls are `crossSessionInbound` (`accept`/`hold`/`refuse`, settable via `--settings`). With no value set, a prompting session accepts messages. A message can't approve permissions or change config. Max 50 queued, 100 held. `notify_when_idle` gives one idle notice. Auth line `{"type":"auth","token":…}` documented; message line format **undocumented**.
- **Doc, Stop hook:** `decision: "block"` + `reason`, or `additionalContext`, continues the turn. Input has `stop_hook_active`; cap of 8 consecutive continuations (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`). Only runs at turn end, so it can't wake an idle session.
- **Doc, Channels:** research preview. An MCP server pushes `notifications/claude/channel`; custom channels need `--dangerously-load-development-channels`; optional permission relay. Not part of the first release (PLAN.md §3).
- **Tested:** — (M0.5: delivery, acknowledgment, latency for each)
- **Fallback:** native `attach`.

### C11 Completion and attention signals
- **Doc:** `Stop` hook per turn; `Notification` types `agent_needs_input` and `agent_completed` fire **only while agent view is open in a terminal**; `SessionStart` `source` = `startup|resume|clear|compact|fork`.
- **Tested:** —
- **Fallback:** poll `agents --json`.

### C12 Usage and quota telemetry
- **Doc, status line JSON:** `cost.total_cost_usd` (client-side estimate), `context_window.*` (current context, not cumulative), `rate_limits.five_hour|seven_day.used_percentage|resets_at` (optional), `prompt_cache`, `transcript_path`. Runs on UI events or `refreshInterval`.
- **Doc, OpenTelemetry:** `CLAUDE_CODE_ENABLE_TELEMETRY=1` with `OTEL_METRICS_EXPORTER` / `OTEL_LOGS_EXPORTER` = `otlp|console|…`. Project/local settings can only turn exporters *off*, so Baton enables them via the spawn environment or `--settings`.
- **Doc, headless:** `-p --output-format json` reports `total_cost_usd`. Not applicable to `--bg`.
- **Assumption:** transcript JSONL carries per-message `usage`; the format is undocumented, so treat it as a versioned, fallible adapter.
- **Tested:** — (M0.6: which of these produce data while detached)
- **Fallback:** show "telemetry unavailable" and never show 0.

### C13 Prompt-cache flags
- **Doc/Probed:** `--exclude-dynamic-system-prompt-sections` (default system prompt only) and `--system-prompt-snapshot on|off`.
- **Tested:** — (deferred to M3; experiment only)

### C14 Auth boundary
- **Doc:** OAuth is "intended exclusively for" subscription purchasers' "ordinary use of Claude Code and other native Anthropic applications". Developers building products should use API keys and "may not collect, store, or intermediate Claude.ai credentials". This doesn't prevent "an end user from signing in to the unmodified Claude Code binary with their own Claude subscription".
- **Baton rule:** spawn the unmodified CLI under the owner's own login. Never read or store credentials. Re-check before any distribution. (Not legal advice.)

### C15 Workspace trust and failure paths
- **Doc:** `--bg` checks trust for its directory before starting. From a terminal it shows the dialog; "where no dialog can appear, such as in a script", it fails with `Workspace not trusted` (v2.1.281+).
- **Probed:** no CLI flag or subcommand sets trust. Trust lives in `~/.claude.json` (`projects.<path>.hasTrustDialogAccepted`), which Baton must not edit.
- **Tested (2026-10-04, `spikes/m0.2-lifecycle.sh spawn-a`):** from a non-TTY shell, `claude --bg` in an untrusted repo exits 1 with ``Workspace not trusted. Run `claude` in <dir> once and accept the trust prompt, then retry.`` No session starts and no quota is used. Trust on `~/projects` (a non-git parent) did **not** extend to a new git repo at `~/projects/baton-sandbox`.
- **Tested (M0.2 `spawn-b`):** after the owner trusted the repo, `--bg` started in Baton-made linked worktrees both inside the repo (`.claude/worktrees/…`) and in a sibling directory.
- **Pending:** missing supervisor and cancellation (M0.7).
- **Fallback:** Baton checks trust before dispatch, and its doctor tells the owner to run `claude` in the repo once.

[agent-view]: https://code.claude.com/docs/en/agent-view
[cli-reference]: https://code.claude.com/docs/en/cli-reference
[hooks]: https://code.claude.com/docs/en/hooks
[cross-session-messaging]: https://code.claude.com/docs/en/cross-session-messaging
[channels]: https://code.claude.com/docs/en/channels
[channels-reference]: https://code.claude.com/docs/en/channels-reference
[statusline]: https://code.claude.com/docs/en/statusline
[monitoring-usage]: https://code.claude.com/docs/en/monitoring-usage
[worktrees]: https://code.claude.com/docs/en/worktrees
[headless]: https://code.claude.com/docs/en/headless
[legal-and-compliance]: https://code.claude.com/docs/en/legal-and-compliance
