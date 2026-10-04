# Baton architecture (M0 output)

**Status:** draft for owner approval, 2026-10-04. Built on Claude Code **2.1.289** on WSL2. Every claim about Claude Code below points to a finding (F1–F20) or capability (C1–C15) in [`compat-record.md`](compat-record.md), where the evidence and the spike that produced it live. The product scope is [`PLAN.md`](../PLAN.md) Part B; this document only says *how* Baton meets it on top of Claude Code.

## 1. Shape

One Rust binary, `baton`, in four roles:

| Role | Command | Job |
|---|---|---|
| Daemon | `baton daemon` | Owns all state (SQLite), dispatches and watches workers, answers hooks, receives telemetry. Survives the TUI closing. |
| TUI | `baton` | A replaceable view: what needs me, what's progressing, what's ready to review. |
| CLI | `baton <verb> --json` | Same actions, scriptable. |
| Hook | `baton hook <event>` | Tiny entry point Claude Code runs for each hook event; talks to the daemon over a local Unix socket. |

```
 owner ── TUI / CLI ──► baton daemon ──── SQLite (tasks, attempts, decisions, audit log)
                          │   ▲  ▲
          claude --bg ... │   │  └── OTLP/HTTP-JSON (usage)   ◄── worker
          claude stop/rm  │   │                                  (one claude process
          agents --json   │   └── Unix socket ◄── baton hook ◄──  per worker, in a
                          ▼                                       Baton-made worktree)
                  Claude supervisor (on-demand; not Baton's to manage, F20)
```

Baton never reads or stores Claude credentials; it only runs the unmodified `claude` CLI under the owner's own login (C14).

## 2. How Baton drives a worker

**Prepare.** Baton checks the repo is trusted (`claude --bg` from a script refuses untrusted folders; the owner trusts each repo once with `claude` in a terminal, F8). It creates the worker's worktree itself with `git worktree add` (D1) and writes the worker's settings file (hooks, deny rules, OTel env, status line).

**Dispatch.** From inside that worktree:

```
claude --bg "<task prompt>" --name baton-<task>-<attempt> --agents '<inline JSON>' --agent baton-<role> \
  --settings <baton settings file> --permission-mode <D2> --allowedTools <scope> --model <model>
```

- The prompt goes first: variadic flags would swallow it (C1).
- `--session-id` is ignored with `--bg`; Baton reads the id from the `backgrounded · <id>` line and the UUID from `claude agents --json` (F11).
- `--agents` must be inline JSON for `--bg` (F12). Its "no agent named …" warning is wrong; Baton confirms the role from the `SessionStart` hook's `agent_type` instead (F6, F12).
- A custom agent's `tools` must include `ToolSearch` if it uses MCP tools (F13).
- Never `--bare`: it drops hooks, `CLAUDE.md` and the messaging inbox (F5).

**Observe.** Two sources, never the `state` field for "finished" (F19):
- `claude agents --json --all` (polled) for liveness: `pid`, `status` (`busy`/`waiting`/`idle`), `waitingFor`, `cwd` (C2).
- Hook events pushed by `baton hook`: `SessionStart` (role, model, transcript path), `PreToolUse`/`PostToolUse`, `PermissionRequest`, `Notification` (`permission_prompt`, `idle_prompt`), `UserPromptSubmit`, `Stop` (with the final message), `SessionEnd` (C11).
- `claude logs` is a lossy screen capture: display-only, sanitized (C5).

**Permissions** (F16, C9). The `PermissionRequest` hook asks the daemon, which answers from the approved policy (routine, in-scope actions) or creates a durable decision for the owner. The hook waits a bounded time (shorter than its timeout) and returns allow, deny or nothing. Nothing means Claude's own prompt stays up, so a lost answer never grants anything. If the owner answers later, Baton stops the worker and resumes it flag-free: Claude records the pending call as "outcome unknown", the worker retries, the request comes back through the hook, and Baton applies the answer **only if the re-asked request matches the one the owner approved** (tool and input); anything else is a new request.

**Guards that must hold even if Baton is down.** Hooks fail open: a crashed command hook or an unreachable HTTP hook is a non-blocking error. So each hard rule is enforced twice:
- a native `permissions.deny` rule in the settings file (Claude enforces it; F2), e.g. `Bash(git push:*)`, `Bash(gh pr:*)`;
- a `PreToolUse` hook that evaluates the same policy locally (no daemon needed) and records every attempt (F17), plus a role guard that denies every tool when `agent_type` isn't the expected role (F6).

**Steering** (F18, C10).
- Busy worker: Baton queues the instruction; the `PostToolUse` hook delivers it as `additionalContext` after the running tool finishes, or the `Stop` hook at turn end. The hook's own log is the delivery receipt.
- Idle worker: `claude stop`, then `claude --bg "<instruction>" --resume <uuid>` with **no other flags** (~2 s to delivery, same session).
- `SendMessage` works too but needs a Claude session as sender; not used in M1.

**Resume rules** (F1, F9, F10, F14). Resume only flag-free (any flag starts a copy), only after confirming the process isn't running (a running or idle-but-alive session also copies), and never with `respawn` (it can re-run the original prompt). Copies can write into the original's worktree, so any unexpected new id is stopped at once. To change a worker's hooks, MCP or policy between attempts, Baton edits its own settings files: file-path config is re-read on resume.

**Verify** (F15). Workers' reports are not evidence: a worker whose push was denied still said "DONE". Baton snapshots the worktree (commit or tree hash), runs the acceptance checks itself, and ties results to that exact candidate (PLAN §6).

**Clean up** (C5). Baton removes its own worktrees after checking for unmerged work; `claude rm` never deletes worktrees it didn't create, and refuses to drop unpushed or uncommitted work in its own.

## 3. Usage and quota (F7, C12)

- **Tokens and cost:** OpenTelemetry from each worker (`--settings` `env`, `http/json`) to a receiver inside the daemon. Metrics are per-process deltas under one `session.id`: Baton sums them. Events carry the owner's email and account ids: Baton drops those fields before storing.
- **Quota:** only the status line has `rate_limits.five_hour`/`seven_day`. Baton's status-line command (`baton hook statusline`) forwards it; it refreshes while a worker idles and stops when the process does, so Baton shows its age. Its cost field is cumulative per session: never summed.
- **Cross-check:** transcript `usage`, de-duplicated by message id. Undocumented format, so a versioned adapter that may fail.
- Missing data shows as unknown, never zero.

## 4. State (M1 subset of PLAN §4)

SQLite with an append-only audit log. Separate identities: **mission** (later), **task**, **attempt** (one dispatch; config refs and usage), **backend session** (short id + UUID; may change only on an unexpected copy), **workspace** (worktree path, branch, base), **candidate** (exact tree/commit under review), **verification** (check results for one candidate), **decision** (durable request: permission or product choice, with status). Baton derives its own task states from hooks and polling (queued, running, waiting for permission, waiting for input, verifying, review-ready, accepted, failed, cancelled, reconciling); Claude's `state` field is not used for them.

On daemon start, Baton reconciles: every attempt it thinks is live is checked against `claude agents --json --all`, its worktree and its last hook event; anything ambiguous becomes `reconciling` until evidence settles it (PLAN §4, F20).

## 5. Known limitations

1. **Claude's own permission prompt runs alongside Baton's hook.** `waitingFor: "permission prompt"` shows as soon as a request starts; if the owner is attached, whichever of them answers first wins (F16).
2. **Busy steering waits for the running tool;** a long command means a long wait. Idle steering restarts the worker process (F18).
3. **Each repo needs one interactive trust step** by the owner (F8).
4. **Quota data exists only while a worker process lives;** idle workers are stopped by the supervisor after about an hour, so quota can go stale (C6, F7).
5. **Workers don't auto-commit in Baton-made worktrees.** Observed, not documented; if Claude changes this, Baton's snapshot step still works but must not assume a clean tree (C4).
6. **Inline agent JSON is visible in the process list** (`ps`) to the local user.
7. **Undocumented surfaces** (transcript format, `claude logs`, socket messages) are adapters that may break; documented hooks, CLI output and OTel are the contract.
8. **Machine shutdown is untested;** the docs say sessions show `failed` and can be resumed within 48 h (C6).
9. **Evidence base is narrow:** one machine (WSL2), one Claude Code version, Haiku workers, Pro plan, small samples. Spike scripts double as compatibility tests and must be re-run on every Claude Code upgrade before Baton trusts the new version.
10. **Agent view and channels are research previews;** Baton depends on neither, but the background-session CLI it uses is documented alongside them and may change.

## 6. Decisions for the owner

Each card: options, ★ recommendation, and whether it's easy to reverse.

**D1. Where worker worktrees live.**
- ★ **Baton creates them under `<repo>/.claude/worktrees/baton-<task>-<attempt>`.** Trusted with the repo, in the folder Claude itself uses for worktrees (Baton adds it to the repo's `.gitignore` if missing, or the main checkout shows an untracked `.claude/`), one known path per attempt, no auto-commit (F3, F8).
- A sibling directory (`<repo>-baton/…`). Also works and keeps the repo folder clean, but scatters directories.
- Let Claude create them. Rejected: lazy path, auto-commit, `rm` semantics tied to Claude.
- *Reversible:* yes, a path setting.

**D2. Worker permission mode.**
- ★ **`acceptEdits` + Baton's policy hook.** Edits inside the worktree go through; everything else reaches Baton's `PermissionRequest` hook, which auto-answers in-scope actions and asks the owner about the rest.
- `default` + policy hook. Every edit also goes through Baton: maximum audit, more hook round-trips.
- `auto`. Claude's classifier decides; opaque and costs extra requests. Not for M1.
- *Reversible:* yes, per task.

**D3. How roles are defined.**
- ★ **Inline `--agents` JSON generated by Baton,** plus the `PreToolUse` role guard. Saved with the session, so it can't vanish before a resume (F6, F12).
- Project files in `.claude/agents/`. Readable and editable, but a missing file silently widens a resumed worker.
- *Reversible:* yes.

**D4. Hook transport.**
- ★ **Command hooks running `baton hook <event>`,** which talk to the daemon over a Unix socket, with the guards evaluated locally so they fail closed in practice, and the native deny rules as backstop.
- HTTP hooks straight to the daemon. No process per event, but fail open when the daemon is down; fine later for purely informational events.
- *Reversible:* yes, internal.

**D5. Steering.**
- ★ **Hooks for busy workers, stop + flag-free resume for idle ones** (F18). Pure CLI, no extra model calls.
- Add a `SendMessage` relay (a small Claude session) for near-instant idle delivery. Costs tokens per message; maybe later.
- *Reversible:* yes.

**D6. Usage sources.**
- ★ **OTel into the daemon for tokens and cost, the status line for quota, the transcript only as a cross-check** (F7).
- Transcript only. No receiver needed, but no quota and an undocumented format.
- *Reversible:* yes.

**D7. M1 scope.**
- ★ **PLAN.md's M1 as written,** built on the choices above: daemon-owned SQLite state, quick task intake, one worker, independent checks on an exact candidate, native attach, an 80×24 TUI, explicit acceptance, restart recovery, usage and decision instrumentation. First slice: `baton task "<goal>"` → one worker → checks → review screen.
- Shrink M1 to CLI-only (no TUI) to reach a working loop sooner; add the TUI in M1.5.
- *Reversible:* scope can be cut later; building the wrong thing costs time.
