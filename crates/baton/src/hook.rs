//! `baton hook <Event>`: what Claude Code runs for each hook event of a Baton
//! worker. Guards are decided here, without the daemon, so they hold while it is
//! down (D4); the event is then reported to the daemon, best effort.

use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::client;
use crate::paths::Paths;
use crate::protocol::{Request, Response};

/// Hooks run inline with the worker's tool calls; never hold it up for long.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);

/// Handles one event; returns the JSON to print for Claude Code, if any. Without
/// `paths` the guards still apply; only the report to the daemon is skipped.
pub fn run(paths: Option<&Paths>, attempt: i64, role: &str, event: &str, stdin: &str) -> Option<Value> {
    let input: Value = serde_json::from_str(stdin).unwrap_or(Value::Null);
    let denied = (event == "PreToolUse").then(|| guard(&input, role)).flatten();
    let request = Request::Hook { attempt, event: event.into(), input: summarize(&input), denied: denied.clone() };
    let reply = paths.map(|p| client::call_with_timeout(p, &request, DAEMON_TIMEOUT));
    if let Some(reason) = denied {
        return Some(json!({ "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }}));
    }
    match reply? {
        Ok(Response::Hook { output }) => output,
        Ok(_) => None,
        Err(e) => {
            eprintln!("baton hook: {e:#}");
            None
        }
    }
}

/// The reason to deny a tool call, if any rule forbids it.
pub fn guard(input: &Value, role: &str) -> Option<String> {
    let agent = input["agent_type"].as_str();
    if agent != Some(role) {
        // A missing or different role means broader tools than Baton approved (F6).
        return Some(format!(
            "Baton role guard: this session runs as {}, not {role}, so Baton blocks its tools.",
            agent.unwrap_or("no agent")
        ));
    }
    let command = input["tool_input"]["command"].as_str().unwrap_or_default();
    if input["tool_name"] == "Bash" && publishes(command) {
        return Some(
            "Baton policy: pushing or publishing needs the owner's approval. Leave your changes in the worktree; Baton snapshots them."
                .into(),
        );
    }
    None
}

/// Whether a shell command runs `git push`, `gh pr` or `gh release`, anywhere in it.
/// A heuristic over the command text; the native deny rules back it up.
fn publishes(command: &str) -> bool {
    let tokens: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || ";&|()`'\"".contains(c))
        .filter(|t| !t.is_empty())
        .collect();
    tokens.iter().enumerate().any(|(i, t)| match t.rsplit('/').next() {
        Some("git") => git_subcommand(&tokens[i + 1..]) == Some("push"),
        Some("gh") => matches!(tokens.get(i + 1), Some(&("pr" | "release"))),
        _ => false,
    })
}

fn git_subcommand<'a>(rest: &[&'a str]) -> Option<&'a str> {
    let mut it = rest.iter();
    while let Some(t) = it.next() {
        match *t {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => {
                it.next();
            }
            t if t.starts_with('-') => {}
            t => return Some(t),
        }
    }
    None
}

/// What the daemon needs from a hook input. File contents and tool output stay
/// behind; long text is cut, so messages stay small (PLAN §10).
fn summarize(input: &Value) -> Value {
    const FIELDS: [&str; 12] = [
        "session_id",
        "transcript_path",
        "agent_type",
        "hook_event_name",
        "source",
        "model",
        "permission_mode",
        "tool_name",
        "notification_type",
        "message",
        "reason",
        "stop_hook_active",
    ];
    let mut out = Map::new();
    for field in FIELDS {
        if let Some(v) = input.get(field) {
            out.insert(field.into(), v.clone());
        }
    }
    if let Some(command) = input["tool_input"]["command"].as_str() {
        out.insert("command".into(), truncate(command, 4 << 10).into());
    }
    if let Some(message) = input["last_assistant_message"].as_str() {
        out.insert("last_assistant_message".into(), truncate(message, 64 << 10).into());
    }
    Value::Object(out)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(command: &str) -> Value {
        json!({ "agent_type": "baton-worker", "tool_name": "Bash", "tool_input": { "command": command } })
    }

    #[test]
    fn push_and_publish_are_denied_wherever_they_appear() {
        for command in [
            "git push",
            "git push origin main",
            "git add -A && git commit -m x && git push",
            "cd /x; git -C /repo push",
            "/usr/bin/git -c user.name=x push --force",
            "bash -c 'git push'",
            "gh pr create --fill",
            "gh release create v1",
        ] {
            assert!(guard(&bash(command), "baton-worker").is_some(), "{command}");
        }
        for command in ["git status", "git log --grep push", "echo push", "cargo test", "gh issue list", "git commit -m 'push later'"] {
            assert!(guard(&bash(command), "baton-worker").is_none(), "{command}");
        }
    }

    #[test]
    fn role_guard_blocks_every_tool_of_another_role() {
        let write = json!({ "agent_type": "baton-worker", "tool_name": "Write", "tool_input": { "file_path": "/w/a" } });
        assert!(guard(&write, "baton-worker").is_none());
        let mut no_agent = write.clone();
        no_agent.as_object_mut().unwrap().remove("agent_type");
        assert!(guard(&no_agent, "baton-worker").unwrap().contains("runs as no agent"));
        assert!(guard(&write, "baton-reviewer").unwrap().contains("not baton-reviewer"));
    }

    #[test]
    fn summaries_drop_contents_and_cut_long_text() {
        let input = json!({
            "session_id": "abcd1234-x", "agent_type": "baton-worker", "tool_name": "Write",
            "tool_input": { "file_path": "/w/a", "content": "secret file contents" },
            "tool_response": { "stdout": "lots" },
            "last_assistant_message": "é".repeat(40_000),
        });
        let s = summarize(&input);
        assert_eq!(s["session_id"], "abcd1234-x");
        assert!(s.get("tool_input").is_none() && s.get("tool_response").is_none());
        let msg = s["last_assistant_message"].as_str().unwrap();
        assert!(msg.len() <= 64 << 10 && msg.chars().all(|c| c == 'é'));
    }

    #[test]
    fn guards_hold_without_a_daemon() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths { home: home.path().into() };
        let out = run(Some(&paths), 1, "baton-worker", "PreToolUse", &bash("git push").to_string()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(run(Some(&paths), 1, "baton-worker", "PreToolUse", &bash("ls").to_string()).is_none());
        assert!(run(None, 1, "baton-worker", "PreToolUse", &bash("git push").to_string()).is_some());
        assert!(run(Some(&paths), 1, "baton-worker", "Stop", "{}").is_none());
    }
}
