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
| Claude Code | 2.1.289 (M0 spikes, M1 runs 1–5). 2.1.292 since 2026-10-06: only the M1.9 demo (smoke run 6) ran on it; the spikes have not been re-run |
| OS | Linux 6.18.33.2-microsoft-standard-WSL2 |
| tmux | 3.6 |
| git | 2.53.0 |
| Rust | 1.99.0 (`~/.cargo/bin`) |
| Auth | claude.ai subscription (OAuth), owned by the CLI |
| Background supervisor | on-demand "transient" daemon; `claude daemon status` reports pid, version, socket dir `/tmp/cc-daemon-<uid>/…` (Probed) |

Doc pages: [agent-view], [cli-reference], [hooks], [cross-session-messaging], [channels], [channels-reference], [statusline], [monitoring-usage], [worktrees], [headless], [legal-and-compliance].

## Findings that change the plan

1. **`respawn` is not resume.** `respawn` restarts a session's process (for a new binary). If no transcript is on disk it **re-runs the original prompt**, which can duplicate work. Resume is `claude attach <id>` or `claude --resume <uuid> --bg` **with no other flags** (see 9). (Doc, Probed, Tested)
2. **Background sessions publish on their own.** Changelog v2.1.198: a session isolated in a worktree "commits, pushes its own isolated branch … and opens a draft pull request when it finishes instead of asking first." This conflicts with Baton's policy (no push without the owner). Tested: it's inconsistent (with a remote and push allowed, one worker didn't even commit; another tried `git add && git commit && git push` in one command). A `--settings` deny rule `Bash(git push:*)` refused that whole compound command and nothing reached the remote. (Doc, Tested)
3. **Baton should create the worktree and dispatch inside it.** Claude's own worktree is created lazily, shows up in `agents --json` `cwd` only while the process lives, and comes with a self-made commit. A session dispatched inside a Baton-made worktree stays there, nests nothing, leaves its edits uncommitted for Baton to snapshot, and `claude rm` never deletes that worktree. (Tested)
4. **Steering has a documented native path.** Cross-session messaging works for background sessions: an idle session starts a new turn, a busy one reads it between tool calls. The socket's auth line is documented but **the message line format is not**, so posting raw to the socket stays an Assumption. (Doc)
5. **`--bare` is unusable for workers.** It skips hooks, CLAUDE.md and the messaging inbox. (Doc, Probed)
6. **A missing role widens a resumed session silently.** With a project-file agent deleted before a flag-free resume, the worker came back with the full default tool set and used Bash, which the agent never allowed. No warning appeared in the dispatch output, transcript or screen, and the recorded system prompt still made it *sound* like the agent. The only signal is `SessionStart` `agent_type` (`None` instead of the agent). Baton must check the agent exists before resuming, and treat an `agent_type` mismatch as a role failure: stop the worker. Inline `--agents` are saved with the session, so they can't go missing this way. (Doc, Tested)
7. **Telemetry works while detached, and the sources agree.** OpenTelemetry (metrics and `api_request` events), the transcript and the status line all report the same tokens and cost for a detached worker. Only the status line has quota (`rate_limits.five_hour`/`seven_day`), and it keeps refreshing while the worker idles, until the process stops. Count with care: OTel sends per-process deltas (sum them); the status line's cost is a session-cumulative snapshot (don't sum); the transcript needs de-duplication by message id. OTel events carry the owner's email and account ids, so Baton strips them before storing. (Tested)
8. **Every target repo must be trusted interactively once.** A script can't accept the trust dialog, and trusting a parent folder didn't cover a new repo inside it. Linked worktrees of a trusted repo pass, wherever they live. Baton needs a one-time "trust this repo" step for the owner. (Tested)
9. **Any flag on `--resume` makes a copy.** `--resume <uuid> --bg` plus *any* other flag (`--model`, `--permission-mode`…) starts a copy with a new id: "keeps its own saved options, so the flags you passed started a copy". With no flags it wakes the same session with its saved options. Resuming a running session also copies. (Tested)
10. **Copies are dangerous.** A copy carries the full conversation, starts in the original's dispatch directory, and edited the **original's** worktree. Baton must never create copies by accident: resume flag-free, and only after confirming the session isn't running. (Tested)
11. **`--bg` ignores `--session-id`.** It prints a warning and assigns its own id; the short id is the first 8 characters of the session UUID. Baton reads identity from the dispatch output. (Tested)
12. **`--bg` takes `--agents` only as inline JSON**, and its pre-check warns `no agent named '<x>' — spawning with default template` even though the inline agent *is* applied (`agent_type` in hooks, agent prompt as the whole system prompt). Baton must confirm the role from `SessionStart` `agent_type`, not trust or fear the warning. (Tested)
13. **A custom agent needs `ToolSearch` to reach MCP tools.** MCP tool search is on by default, so tool definitions are deferred; without `ToolSearch` in the agent's `tools`, the model never saw `mcp__baton__ping`. (Doc, Tested)
14. **Config passed as file paths is re-read on resume.** `--settings`/`--mcp-config` file contents fixed between stop and a flag-free resume took effect, with no copy. Baton can adjust hooks/MCP between attempts by editing its own files. (Tested)
15. **Workers don't report their own failures reliably.** A worker whose `git push` was denied (`is_error: true`) still replied just "DONE". Baton must verify outcomes itself. (Tested)
16. **Baton can answer permission requests, safely bounded.** A `PermissionRequest` hook that waits on Baton's inbox gets allow/deny honoured; past its timeout it's killed and the native prompt remains, so a late or lost answer never grants anything. A stuck prompt can be recovered without a terminal by stop + flag-free resume, which re-asks through the hook. The `waitingFor: "permission prompt"` row appears as soon as the request starts, even while Baton's hook is still deciding. (Tested)
17. **A `PreToolUse` deny hook is the visible push block.** Unlike a deny rule, Baton records every attempt; the worker sees the reason, labelled "hook error". (Tested)
18. **Baton can steer workers with hooks and the CLI alone.** Busy: a `PostToolUse` hook reading Baton's queue delivers at the next tool boundary (wait ≈ the running tool), a `Stop` hook at turn end; the hook's own log is the delivery receipt. Idle: `claude stop` + flag-free resume with the instruction as the prompt (≈2 s to delivery, same session). `SendMessage` also works for both and is near-instant when idle, but needs a Claude session as the sender. Resuming a live idle session is **not** a route: it copies. (Tested)
19. **Don't read "finished" from `state`.** One idle worker showed `working`, then `blocked`, never `done`. Use `status`, the `Stop` hook, and `Notification` `idle_prompt` (60 s after idle). (Tested)
20. **Workers outlive the supervisor.** Each background session is its own process: a graceful `--keep-workers` stop or a `kill -9` of the supervisor didn't interrupt a busy worker, and the next `claude agents`/`claude --bg` call started a new supervisor that re-adopted it with the same pid. Baton needn't babysit the supervisor; after any hiccup it re-reads `claude agents --json --all`. (Tested)
21. **An idle session makes one billed request no transcript counts.** About 3 min after a turn ends, Claude Code sends an `away_summary` request (OTel `api_request`, same `session.id`) and writes it to the transcript as `system`/`away_summary` with no `usage`. Seen in smoke run 5 (2.1.289, +196 s) and run 6 (2.1.292, +183 s; 79 in / 326 out). Baton matches transcript `requestId`s against OTel and shows the rest as side requests. (Tested)

## Capabilities

### C1 Spawn
- **Doc:** `claude --bg [--name N] "<prompt>"` returns immediately and prints the short id. `--bg` with `-p` is rejected (v2.1.198). Background sessions inherit `PATH` and settings from the dispatching shell.
- **Probed:** `--bg`, `-n/--name`, `--session-id <uuid>` exist in help.
- **Tested (2026-10-04, `spikes/m0.2-lifecycle.sh`):** returns in 0.66–0.85 s (n=7). Prints `backgrounded · <id> · <name>` plus hint lines, with ANSI colour even when stdout isn't a TTY. `--session-id` is ignored with `warning: --bg manages the session id; ignoring --session-id`. The row appears in `agents --json` immediately as `working`, before `pid`/`status` exist. A `PostToolUse` "Tip: Run /ultrareview" message appeared; the owner's settings have no hooks, so it comes from Claude Code itself.
- **Fallback:** strip ANSI, parse the id from the `backgrounded · <id>` line (a `note:` line can name another session first), then match it in `agents --json`.

### C2 List and state
- **Doc:** `claude agents --json [--all] [--cwd <path>]`. `state`: `working` · `blocked` · `done` · `failed` · `stopped`. `status` (process alive): `busy` · `waiting` · `idle`. `waitingFor`: `permission prompt` · `input needed` · `sandbox request` · `worker request` · `dialog open`.
- **Probed:** fields `id` (8 chars), `sessionId` (UUID), `name`, `kind` (`background`), `pid`, `cwd`, `startedAt` (epoch ms), `state`, `status`. Seen: `working`, `blocked`, `done`; `status` missing on finished sessions. Not yet seen: `waitingFor`, `failed`, `stopped`.
- **Tested:** seen `working` → `done`/`idle`, and `blocked`/`waiting` with `waitingFor: "permission prompt"`. `cwd` follows the live process into its worktree. `startedAt` changes with each new process, so it isn't an identity. After `stop`, a `done` session stays `state: done` (not `stopped`), loses `pid`/`status`, and `cwd`/`startedAt` revert to dispatch-time values. Auto-naming can rename a session (a copy became "file append operation"). An empty interactive-then-backgrounded session showed `blocked`/`idle` with no `waitingFor`. M0.3: a custom-agent worker that finished with "DONE2" (not a question) still showed `blocked`/`idle`, so `state` isn't a reliable "finished" signal.
- **Fallback:** poll the CLI and treat unknown values as `unknown`. Never read `~/.claude/jobs/*/state.json` (internal).

### C3 Identities
- **Doc:** short id is for `attach`/`logs`/`stop`/`respawn`/`rm`; UUID is for `--resume`. `--resume <uuid> --bg` continues under the same id, or starts a copy and prints a `note:` line when the session is already running. `--resume <short-id>` starts a copy. `--fork-session` always copies.
- **Tested:** short id = first 8 chars of the UUID (7 of 7 sessions seen). `--resume <uuid> --bg` with no flags: `note: woke session 022db4c9 with its saved options (--name, --permission-mode, --allowedTools, --model)`, same id, new pid, back in its worktree `cwd`. With any flag: `note: background session … keeps its own saved options, so the flags you passed started a copy as <new>`. While running: `note: session … is already running in the background, so this started a copy as <new>`. A copy carries the conversation but not the worktree `cwd`.
- **Fallback:** Baton stores backend identity per attempt, resumes flag-free only after checking the session isn't running, and treats any new id in the output as a copy to stop.

### C4 Workspace / worktree
- **Doc:** lazy worktree under `.claude/worktrees/`, skipped when already in a linked worktree, outside git, or with `worktree.bgIsolation: "none"`. A `WorktreeCreate` hook replaces git's default. Deleting a session keeps worktrees that hold unpushed commits.
- **Tested:** Claude-made: `.claude/worktrees/<adjective-noun>` on branch `worktree-<name>`, git-locked with reason `claude session <name> (pid … start …)`, and the session committed its edit itself (authored as the owner's git identity, no remote so no push seen). The main checkout then shows an untracked `.claude/`. Baton-made (`git worktree add`, both inside `.claude/worktrees/` and in a sibling directory): the session stayed in it, created no nested worktree, and left its edit **uncommitted**. M0.3: Claude makes its own worktree by calling the `EnterWorktree` tool (seen in `PreToolUse`), after its first write in the shared checkout is refused. `rm` also keeps a worktree with only untracked files, and removes it cleanly once its commits are on the remote.
- **Fallback:** a `WorktreeCreate` hook reports the path to Baton.

### C5 Attach, logs, stop, delete
- **Doc/Probed:** `attach <id>` takes over the terminal (fullscreen); `←`/`/exit` detaches, `Ctrl+Z` drops to the shell; detaching never stops the session. Unsent input blocks detach. `logs <id>` prints recent terminal output. `stop|kill <id>` keeps the conversation. `rm <id>` deletes the session and its worktree when safe (`--discard-unpushed`, `--force-remove-worktree`).
- **Tested:** `logs` is a raw screen capture (ANSI, no newlines, partial redraws such as "Commtted"), so it's display-only after sanitizing. It does show pending permission dialogs in full. `stop` prints `stopped <id>` and `worktree retained at <path>`. `rm` works on live idle sessions. It refused a Claude-made worktree with uncommitted changes (`kept … Deleting it would lose them`) and then with unpushed commits (`kept … 2 unpushed commits … claude rm <id> --discard-unpushed <sha>@<worktree-id>`). With that flag it removed the worktree **and its branch**. It removed sessions in Baton-made worktrees and left those worktrees untouched.
- **Tested (2.1.292, smoke run 6, `demo/m1-demo.sh`):** `attach` from Baton's TUI in an 80×24 tmux pane showed the idle worker's conversation, its handoff, and Baton's status line. `Ctrl+Z` ended `claude attach` and returned to Baton's review screen, and the session stayed idle and resumable. At 80 columns Claude shows "Resize your terminal to at least 110 columns to show the diff panel".
- **Fallback:** Baton cleans up its own worktrees; it treats `rm`'s `kept` output as "work at risk" and asks the owner.

### C6 Supervisor and machine restart
- **Doc:** a supervisor owns sessions; it stops idle processes after ~1 h unless pinned. Shutdown stops sessions: within 48 h they show `failed` and restart from where they left off; after that, `stopped`. Sleep is survived.
- **Probed:** "Service install is disabled in this version — the daemon runs on demand and exits when the last client disconnects."
- **Probed (M0.7):** `claude daemon status` also reports `bg workers: <n> running (control.sock), <n> in roster.json`. One supervisor serves every background session on the machine, including the owner's own and this build session.
- **Tested (2026-10-04, `CONFIRM=yes spikes/m0.7-failures.sh supervisor`, run detached from the build session):** with a sentinel worker busy (4 × `sleep 30`), `claude daemon stop --any --keep-workers` printed `stopped` / "the next `claude agents` or `claude --bg` will start a new one"; the old supervisor took over 1 s to exit; the worker process lived on. The next `claude agents --json` started a new supervisor that listed the worker with the same pid as `working`/`busy`. `kill -9` on that supervisor also left the worker alive, and the next call started a third supervisor that re-adopted it. The worker finished its task (`done`/`idle`). Two unrelated live background sessions (the build session and one of the owner's) survived both with unchanged pids. Machine shutdown not tested.
- **Fallback:** Baton reconciles from `agents --json --all` on startup; unknown stays unknown.

### C7 Per-session configuration
- **Doc/Probed:** `--agents <json|file>`, `--agent`, `--settings <file|json>` (ranks below managed settings, above user/project/local files), `--setting-sources`, `--mcp-config`, `--strict-mcp-config`, `--plugin-dir`, `--add-dir`, `--append-system-prompt`, `--permission-mode`, `--model`, `--effort`, `--tools`, `--restricted`. `--restricted` drops code-running tools and user/project/local settings, so it doesn't fit workers that run checks.
- **Tested (2026-10-04, `spikes/m0.3-config.sh`):** one `--bg` session took `--agents` (inline only: the file form is refused with "a background session is interactive, and the file form works only with --print") + `--agent` + `--settings` (file) + `--mcp-config` (file) + `--allowedTools` together. Evidence: `SessionStart` `agent_type: baton-worker`; the recorded system prompt was exactly the agent's prompt; `--settings` hooks **and** the repo's `.claude/settings.json` hook both fired; `InstructionsLoaded` showed the project `CLAUDE.md` and `~/.claude/CLAUDE.md` loaded; the reply carried both the agent's and the project's canary tokens; the MCP server got `server/discover` (answered with method-not-found, harmlessly), `initialize`, `tools/list`, `tools/call`. With `ToolSearch` in the agent's tools the worker called `mcp__baton__ping`; without it, it couldn't see the tool. Hooks see `CLAUDE_CODE_MESSAGING_SOCKET` on every event except `SessionEnd`.
- **Fallback:** a project-scoped `.claude/agents/` file in the Baton-made worktree.

### C8 Role restore on resume
- **Doc:** resume restores the agent and its tool restrictions; a missing agent means default tools plus a warning.
- **Tested (M0.3):** flag-free resume printed `woke session … with its saved options (--name, --permission-mode, --allowedTools, --agents, --agent, --settings, --mcp-config, --model)`; on resume, `SessionStart` (`source: resume`) still reported `agent_type` (but no `model`; read the model at `startup`), the tool restriction held (worker listed only Read/Write/Edit/Bash), hooks fired, and the MCP server was relaunched. **M0.7 (`spikes/m0.7-failures.sh`):** worker dispatched with `--agent m07-agent` from a project file (`tools: Read, Write`) reported exactly "Read, Write" and `agent_type: m07-agent`. After `claude stop`, deleting the file and a flag-free resume (note listed `--agent` among the restored options, no warning), `SessionStart` reported `agent_type: None`, the worker listed the full default tool set, ran `echo hi` through Bash, and still ended its reply with the agent's canary (system-prompt snapshot).
- **Fallback:** Baton verifies the agent definition exists before resume and refuses to resume otherwise.

### C9 Permission routing
- **Doc:** `PermissionRequest` fires when Claude is about to prompt, in default mode. A hook can allow or deny (`hookSpecificOutput.decision`); exit code 2 isn't honored. With no decision, a background session shows the prompt and goes `blocked` / `waitingFor: permission prompt`; in sessions that can't prompt, the call is denied. Doesn't fire for sandbox network requests (use Notification `permission_prompt`, which fires after ~6 s). Default command-hook timeout: 600 s. `defer` works only with `-p`. A reply typed in agent view does **not** answer a permission dialog.
- **Tested (M0.2, incidental):** with `--permission-mode acceptEdits` and `--allowedTools "Bash(git add:*) Bash(git commit:*) …"`, a worker still blocked on `cd <worktree> && git add … && git commit …` with "This command changes directory before running a version-control command, which can pick up untrusted hooks or repository configuration from the target directory." The row showed `blocked`/`waiting`/`waitingFor: "permission prompt"` and `logs` showed the dialog. **M0.3:** in a `--bg` session, a logging-only `PermissionRequest` hook fired (with `tool_name`, `tool_input`, `permission_mode`, `permission_suggestions`); with no decision the session showed the prompt and blocked (it did not auto-deny). `Notification` `permission_prompt` followed ~6 s later. A deny rule produced a tool result `Permission to use Bash with command … has been denied.` (`is_error: true`) and fired neither `PermissionRequest` nor `PermissionDenied`.
- **Tested (2026-10-04, `spikes/m0.4-permissions.sh`, default permission mode):** a `PermissionRequest` hook (timeout 10 s) wrote each request to an inbox and waited for an answer. Answered **allow** after 5 s: the command ran 0.4 s later. Answered **deny** with a message: the tool result was `is_error: true` with exactly that message. **No answer**: Claude Code killed the hook at its 10 s timeout (no process left), the native prompt stayed, the session stayed `blocked`/`permission prompt`, and a late "allow" written afterwards changed nothing. While the hook was still waiting, `agents --json` already showed `blocked`/`waiting`/`permission prompt`: the native prompt is up at the same time, and whichever answers first wins. **Recovery without a TTY:** `claude stop` + flag-free resume recorded the pending call as "Tool call interrupted … its outcome is unknown", the worker retried it, the request came back through the hook and Baton's allow let it run. A `PreToolUse` hook returning `permissionDecision: "deny"` blocked `git push` and the worker saw `PreToolUse:Bash hook error: <reason>`.
- **Fallback:** keep the prompt native; Baton shows "attach to answer", or stops and resumes the session so the request comes back to its hook.

### C10 Steering a live session
- **Doc, cross-session messaging (v2.1.224+):** `ListAgents`/`SendMessage` between local sessions, including background ones, over a per-session Unix socket (`CLAUDE_CODE_MESSAGING_SOCKET`). Delivery: idle → new turn; busy → read between tool calls. Inbound controls are `crossSessionInbound` (`accept`/`hold`/`refuse`, settable via `--settings`). With no value set, a prompting session accepts messages. A message can't approve permissions or change config. Max 50 queued, 100 held. `notify_when_idle` gives one idle notice. Auth line `{"type":"auth","token":…}` documented; message line format **undocumented**.
- **Doc, Stop hook:** `decision: "block"` + `reason`, or `additionalContext`, continues the turn. Input has `stop_hook_active`; cap of 8 consecutive continuations (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`). Only runs at turn end, so it can't wake an idle session.
- **Doc, Channels:** research preview. An MCP server pushes `notifications/claude/channel`; custom channels need `--dangerously-load-development-channels`; optional permission relay. Not part of the first release (PLAN.md §3).
- **Tested (2026-10-04, `spikes/m0.5-steering.sh`; Haiku worker, ack = worker creates `ack-<id>.txt`):**

  | Route | Worker | Queued/sent → delivered | Delivered → acted | Delivery signal |
  |---|---|---|---|---|
  | `PostToolUse` hook injects Baton's queued file as `additionalContext` | busy | 5.0 s (rest of the running tool) | 2.0 s | hook logs it; transcript `hook_additional_context` |
  | `Stop` hook injects it as `additionalContext` | turn ending | at turn end | 1.5 s | same; next `Stop` has `stop_hook_active: true` |
  | `SendMessage` from another Claude session | idle | ≤ 0.1 s, starts a new turn | 2.6 s | worker `UserPromptSubmit` |
  | `SendMessage` | busy | queued (`queued_command`), delivered when the running tool finished (~3.8 s) | 2.3 s | worker `UserPromptSubmit` mid-turn |
  | `claude stop` + flag-free `--resume <uuid> --bg "<instruction>"` | idle | 2.1 s (stop 0.4 s + wake 1.5 s) | 1.8 s | `SessionStart` `resume` + `UserPromptSubmit` |
  | flag-free `--resume` while the process is alive and idle | idle | **not a route**: started a copy, which wrote its ack into the original's worktree | | |

  The worker had `crossSessionInbound: "accept"` in `--settings`. A message arrives as `<cross-session-message from="uds:/run/user/<uid>/cc-socks/<pid>.sock" from-name="<sender name>" …>`. The sender's `SendMessage` result only says "in that session's inbox, not yet read". Raw socket posting not tested (format undocumented).
- **Fallback:** native `attach`.

### C11 Completion and attention signals
- **Doc:** `Stop` hook per turn; `Notification` types `agent_needs_input` and `agent_completed` fire **only while agent view is open in a terminal**; `SessionStart` `source` = `startup|resume|clear|compact|fork`.
- **Tested:** —
- **Tested (M0.5):** `Notification` `idle_prompt` fired 60 s after the worker went idle. While idle, the same worker's row read `working`/`idle`, then later `blocked`/`idle`, never `done`; only `status: idle` was consistent.
- **Tested (M0.3):** `Stop` fires at each finished turn with `last_assistant_message`; `claude stop` mid-turn fires `SessionEnd` (`reason: other`) and no `Stop`. `SessionStart` reports `source: startup|resume`.
- **Fallback:** poll `agents --json`.

### C12 Usage and quota telemetry
- **Doc, status line JSON:** `cost.total_cost_usd` (client-side estimate), `context_window.*` (current context, not cumulative), `rate_limits.five_hour|seven_day.used_percentage|resets_at` (optional), `prompt_cache`, `transcript_path`. Runs on UI events or `refreshInterval`.
- **Doc, OpenTelemetry:** `CLAUDE_CODE_ENABLE_TELEMETRY=1` with `OTEL_METRICS_EXPORTER` / `OTEL_LOGS_EXPORTER` = `otlp|console|…`. Project/local settings can only turn exporters *off*, so Baton enables them via the spawn environment or `--settings`.
- **Doc, headless:** `-p --output-format json` reports `total_cost_usd`. Not applicable to `--bg`.
- **Assumption:** transcript JSONL carries per-message `usage`; the format is undocumented, so treat it as a versioned, fallible adapter.
- **Tested (2026-10-04, `spikes/m0.6-telemetry.sh`):** one Haiku worker (4 API requests, then stop + flag-free resume for 1 more), detached throughout, with OTel set via `--settings` `env` (`http/json` to a local receiver, metrics every 5 s, logs every 2 s) and a status line with `refreshInterval: 5`.

  | After | Source | input | output | cache read | cache write | cost USD |
  |---|---|---|---|---|---|---|
  | 1st process | OTel `token.usage`/`cost.usage` metrics | 34 | 539 | 128,446 | 10,698 | 0.03697 |
  | | OTel `api_request` events (4) | 34 | 539 | 128,446 | 10,698 | 0.03697 |
  | | transcript (4 messages in 8 entries) | 34 | 539 | 128,446 | 10,698 | — |
  | | status line `cost.total_cost_usd` | — | — | — | — | 0.03697 |
  | + resumed process | OTel metrics and events (sum of deltas) | 44 | 572 | 163,739 | 11,107 | 0.04149 |
  | | transcript (5 messages in 10 entries) | 44 | 572 | 163,739 | 11,107 | — |
  | | status line (already cumulative) | — | — | — | — | 0.04149 |

  Status line: ran 23 times, including every 5 s while idle; silent while the process was stopped; `rate_limits` present (`five_hour` 16% → 17%, `seven_day` 13%, with `resets_at`); `context_window.used_percentage` 18. OTel: same `session.id` across both processes; metric names `token.usage`, `cost.usage`, `session.count`, `active_time.total`, `lines_of_code.count`, `code_edit_tool.decision`; events include `api_request`, `user_prompt`, `tool_decision`, `tool_result`, `hook_execution_*`; every event carries `user.email`, `user.account_uuid`, `user.account_id`, `organization.id`. No OTel signal carries quota. Each request re-read ~32 K cached tokens (system prompt + tools).
- **Baton rule:** tokens and cost from OTel (sum deltas, keyed by `session.id`) with the transcript as a cross-check; quota from the status line, labelled with its age; anything missing shows as unknown.
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
