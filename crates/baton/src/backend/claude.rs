//! Claude Code backend: drives the unmodified `claude` CLI under the owner's own
//! login (C14). The behaviour relied on here is recorded in docs/compat-record.md.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Backend, DispatchRequest, Liveness, Observation, Resumed, Role, SessionRef};

/// D2: edits inside the worktree go through; everything else reaches Baton's hooks.
pub const PERMISSION_MODE: &str = "acceptEdits";

pub const WORKER_ROLE: &str = "baton-worker";

/// No `EnterWorktree`: the worker stays in Baton's worktree (C4).
const WORKER_TOOLS: [&str; 6] = ["Read", "Write", "Edit", "Bash", "Glob", "Grep"];

/// The agent prompt replaces Claude Code's whole system prompt (F12), so it
/// carries the working rules a worker needs.
const WORKER_PROMPT: &str = "\
You are a Baton worker: a software engineer completing one task in the current git worktree.

Rules:
- Work only inside the current directory. Don't create or switch worktrees or branches.
- Don't commit, push, or open pull requests. Baton snapshots your changes and runs the acceptance checks itself.
- Make the smallest change that fully completes the task, matching the code around it.
- Read before you edit. When you can, run the relevant checks yourself before finishing.
- If the task needs something outside its scope or you're blocked, stop and say exactly what you need.

End your final reply with this handoff:
STATUS: done | blocked
SUMMARY: <one or two sentences>
CHANGED: <files you changed>
CHECKS: <commands you ran and their results, or none>
OPEN: <questions or risks, or none>";

/// The events `baton hook` handles.
const HOOK_EVENTS: [&str; 8] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Seconds Claude gives each hook; a permission hook waits for the owner, so it
/// gets longer than Baton's own wait plus the reply.
const HOOK_TIMEOUT: u32 = 30;
const PERMISSION_HOOK_TIMEOUT: u32 = 60;

/// Native deny rules: enforced by Claude even when Baton's hooks can't run (F2).
const DENY_RULES: [&str; 2] = ["Bash(git push:*)", "Bash(gh pr:*)"];

pub fn worker_role() -> Role {
    let definition = json!({
        WORKER_ROLE: {
            "description": "Baton worker: implements one task in its worktree",
            "prompt": WORKER_PROMPT,
            "tools": WORKER_TOOLS,
        }
    });
    Role { name: WORKER_ROLE.into(), definition: definition.to_string() }
}

/// The worker's `--settings` file: every hook event runs `<hook_command> <Event>` (D4).
pub fn settings(hook_command: &str) -> Value {
    let hooks: serde_json::Map<String, Value> = HOOK_EVENTS
        .iter()
        .map(|event| {
            let timeout = if *event == "PermissionRequest" { PERMISSION_HOOK_TIMEOUT } else { HOOK_TIMEOUT };
            let hook = json!({ "type": "command", "command": format!("{hook_command} {event}"), "timeout": timeout });
            (event.to_string(), json!([{ "hooks": [hook] }]))
        })
        .collect();
    json!({ "hooks": hooks, "permissions": { "deny": DENY_RULES } })
}

pub struct Claude;

impl Backend for Claude {
    fn version(&self) -> Result<String> {
        let out = run(Command::new("claude").arg("--version"))?;
        Ok(out.split_whitespace().next().unwrap_or_default().to_owned())
    }

    fn dispatch(&self, req: &DispatchRequest) -> Result<SessionRef> {
        let out = run(Command::new("claude").current_dir(&req.cwd).args(dispatch_args(req)))?;
        let short_id = backgrounded_id(&out).ok_or_else(|| anyhow!("no session started: {}", out.trim()))?;
        Ok(SessionRef { short_id, uuid: None })
    }

    fn list(&self) -> Result<Vec<Observation>> {
        parse_agents(&run(Command::new("claude").args(["agents", "--json", "--all"]))?)
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        run(Command::new("claude").args(["stop", &session.short_id]))?;
        Ok(())
    }

    fn resume(&self, session: &SessionRef, cwd: &Path, prompt: &str) -> Result<Resumed> {
        let uuid = session
            .uuid
            .as_deref()
            .ok_or_else(|| anyhow!("can't resume {} before its uuid is known", session.short_id))?;
        // Flag-free: any other flag starts a copy (F9).
        let out = run(Command::new("claude").current_dir(cwd).arg("--bg").arg(prompt).args(["--resume", uuid]))?;
        let short_id = backgrounded_id(&out).ok_or_else(|| anyhow!("resume didn't start a session: {}", out.trim()))?;
        Ok(if short_id == session.short_id {
            Resumed::Same(session.clone())
        } else {
            Resumed::Copy(SessionRef { short_id, uuid: None })
        })
    }
}

/// The prompt goes right after `--bg`: variadic flags would swallow it (C1).
fn dispatch_args(req: &DispatchRequest) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--bg".into(), req.prompt.clone().into(), "--name".into(), req.name.clone().into()];
    if let Some(role) = &req.role {
        // Inline only: `--bg` refuses the file form (F12).
        args.extend(["--agents".into(), role.definition.clone().into(), "--agent".into(), role.name.clone().into()]);
    }
    if let Some(settings) = &req.settings {
        args.extend(["--settings".into(), settings.clone().into()]);
    }
    args.extend(["--permission-mode".into(), PERMISSION_MODE.into()]);
    if let Some(model) = &req.model {
        args.extend(["--model".into(), model.clone().into()]);
    }
    args
}

/// The id on the `backgrounded · <id>` line; a `note:` line may name another session first (C1).
fn backgrounded_id(output: &str) -> Option<String> {
    let clean = strip_ansi(output);
    let rest = clean.split("backgrounded · ").nth(1)?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (id.len() == 8).then_some(id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRow {
    id: String,
    session_id: Option<String>,
    name: Option<String>,
    pid: Option<u32>,
    state: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
    cwd: Option<PathBuf>,
}

/// Parses `claude agents --json --all` (C2). Liveness comes from `status`, never
/// from `state` (F19); `state` only tells a just-dispatched row from a stopped one.
fn parse_agents(json: &str) -> Result<Vec<Observation>> {
    let rows: Vec<AgentRow> = serde_json::from_str(json).context("parsing `claude agents --json`")?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let liveness = match (r.status.as_deref(), r.state.as_deref()) {
                (Some("busy"), _) => Liveness::Busy,
                (Some("waiting"), _) => Liveness::Waiting,
                (Some("idle"), _) => Liveness::Idle,
                (Some(other), _) => Liveness::Unknown(other.into()),
                // The row exists before the process reports a status (C1).
                (None, Some("working")) if r.pid.is_none() => Liveness::Unknown("starting".into()),
                (None, _) => Liveness::NotRunning,
            };
            Observation {
                session: SessionRef { short_id: r.id, uuid: r.session_id },
                name: r.name,
                pid: r.pid,
                liveness,
                waiting_for: r.waiting_for,
                cwd: r.cwd,
            }
        })
        .collect())
}

/// Runs a `claude` command; returns stdout and stderr together, since notes and
/// errors can be on either.
fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().context("running `claude`")?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("{} ({})", strip_ansi(&text).trim(), out.status);
    }
    Ok(text)
}

/// Removes CSI escape sequences; `claude` colours output even without a TTY (C1).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_backgrounded_id_after_notes_and_colour() {
        let out = "\x1b[2mnote: session 022db4c9 is already running in the background, so this started a copy as 9f3e11aa\x1b[0m\n\
                   \x1b[1mbackgrounded\x1b[0m · \x1b[36m9f3e11aa\x1b[0m · baton-1-1\n  claude attach 9f3e11aa\n";
        assert_eq!(strip_ansi(out).lines().nth(1).unwrap(), "backgrounded · 9f3e11aa · baton-1-1");
        assert_eq!(backgrounded_id(out).as_deref(), Some("9f3e11aa"));
        assert_eq!(backgrounded_id("Workspace not trusted. Run `claude` in /x once"), None);
    }

    #[test]
    fn dispatch_puts_the_prompt_first() {
        let req = DispatchRequest {
            name: "baton-1-1".into(),
            cwd: "/r/.claude/worktrees/baton-1-1".into(),
            prompt: "Task: x".into(),
            model: Some("haiku".into()),
            role: Some(worker_role()),
            settings: Some("/s/settings.json".into()),
        };
        let args: Vec<String> = dispatch_args(&req).into_iter().map(|a| a.into_string().unwrap()).collect();
        assert_eq!(&args[..4], ["--bg", "Task: x", "--name", "baton-1-1"]);
        assert_eq!(args[5], worker_role().definition);
        assert_eq!(
            &args[6..],
            ["--agent", "baton-worker", "--settings", "/s/settings.json", "--permission-mode", "acceptEdits", "--model", "haiku"]
        );
    }

    #[test]
    fn worker_role_excludes_enter_worktree() {
        let def: Value = serde_json::from_str(&worker_role().definition).unwrap();
        let tools = def[WORKER_ROLE]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t == "Bash"));
        assert!(!tools.iter().any(|t| t == "EnterWorktree"));
    }

    #[test]
    fn settings_route_every_event_and_deny_push() {
        let s = settings("BATON_HOME='/h' '/bin/baton' hook --attempt 7 --role baton-worker");
        let cmd = s["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, "BATON_HOME='/h' '/bin/baton' hook --attempt 7 --role baton-worker PreToolUse");
        assert_eq!(s["hooks"].as_object().unwrap().len(), HOOK_EVENTS.len());
        assert_eq!(s["permissions"]["deny"][0], "Bash(git push:*)");
        // The hook outlasts Baton's wait and the client's reply timeout.
        let permission = s["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"].as_u64().unwrap();
        assert!(permission > crate::hook::PERMISSION_TIMEOUT.as_secs());
        assert!(crate::hook::PERMISSION_TIMEOUT > crate::permission::WAIT);
    }

    #[test]
    fn liveness_comes_from_status() {
        let json = r#"[
            {"id":"aaaaaaaa","sessionId":"aaaaaaaa-1","name":"baton-1-1","kind":"background","pid":12,"cwd":"/w","state":"blocked","status":"waiting","waitingFor":"permission prompt"},
            {"id":"bbbbbbbb","sessionId":"bbbbbbbb-1","state":"working","status":"idle","pid":13},
            {"id":"cccccccc","sessionId":"cccccccc-1","state":"working"},
            {"id":"dddddddd","sessionId":"dddddddd-1","state":"done"},
            {"id":"eeeeeeee","state":"working","status":"thinking","pid":14}
        ]"#;
        let obs = parse_agents(json).unwrap();
        let liveness: Vec<_> = obs.iter().map(|o| o.liveness.clone()).collect();
        assert_eq!(
            liveness,
            [
                Liveness::Waiting,
                Liveness::Idle,
                Liveness::Unknown("starting".into()),
                Liveness::NotRunning,
                Liveness::Unknown("thinking".into())
            ]
        );
        assert_eq!(obs[0].waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(obs[0].session.uuid.as_deref(), Some("aaaaaaaa-1"));
        assert_eq!(obs[0].cwd, Some(PathBuf::from("/w")));
    }
}
