//! Permission requests from workers (D2, F16). Routine in-scope requests are
//! allowed by policy; everything else becomes a durable decision for the owner.
//! The hook waits a bounded time for the answer. A later answer reaches the worker
//! by stop + flag-free resume, and applies only to the exact same request, once.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::backend::{Resumed, SessionRef};
use crate::model::{Decision, Task, TaskState};
use crate::store::{Attempt, NewDecision};
use crate::worker::Ctx;

/// How long a `PermissionRequest` hook waits for the owner; below its timeout in
/// the worker's settings, so Baton answers before Claude gives up on the hook.
pub const WAIT: Duration = Duration::from_secs(50);

/// The prompt for a worker resumed to receive a late answer.
const RESUME_PROMPT: &str =
    "Baton: the owner answered your pending permission request. Retry the interrupted tool call exactly as before, then continue the task.";

/// Hooks waiting on each decision, with the condition variable `decide` signals.
/// A waiting hook takes its answer while holding this lock, so `decide` knows
/// for certain whether a hook delivered it or the worker must be restarted.
#[derive(Default)]
pub struct Waiters {
    waiting: Mutex<HashMap<i64, u32>>,
    changed: Condvar,
}

/// Why policy allows a request without asking, if it does. M1's policy: the
/// task's own checks and plain read-only commands inside the worktree.
pub fn allowed_by_policy(task: &Task, tool: &str, input: &Value) -> Option<&'static str> {
    if tool != "Bash" {
        return None;
    }
    let command = input["command"].as_str()?.trim();
    if task.checks.iter().any(|c| c.trim() == command) {
        return Some("one of the task's checks");
    }
    read_only(command).then_some("read-only command")
}

fn read_only(command: &str) -> bool {
    const PROGRAMS: [&str; 10] = ["ls", "cat", "head", "tail", "wc", "grep", "pwd", "echo", "stat", "file"];
    // No chaining, redirection, substitution, globbing or escapes; no paths outside the worktree.
    if command.contains(|c: char| "|&;<>()$`\\*?\n".contains(c)) {
        return false;
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    if words.iter().any(|w| w.starts_with(['/', '~']) || w.contains("..") || w.starts_with("--output")) {
        return false;
    }
    match words.as_slice() {
        ["git", sub, ..] => matches!(*sub, "status" | "diff" | "log" | "show"),
        [program, ..] => PROGRAMS.contains(program),
        [] => false,
    }
}

/// Handles a `PermissionRequest` hook; returns the decision for Claude Code, or
/// `None` to leave the request to Claude's own prompt.
pub fn on_request(ctx: &Ctx, task: &Task, attempt: &Attempt, input: &Value) -> Result<Option<Value>> {
    let tool = input["tool_name"].as_str().unwrap_or_default();
    let tool_input = &input["tool_input"];
    if tool_input.is_null() {
        // Too large for the hook to forward; Claude's prompt handles it.
        return Ok(None);
    }
    if let Some(rule) = allowed_by_policy(task, tool, tool_input) {
        ctx.store.lock().unwrap().audit_event("attempt", attempt.id, "permission_allowed", &json!({ "tool": tool, "rule": rule }))?;
        return Ok(Some(output("allow", None)));
    }

    let request = request_key(tool, tool_input);
    let existing = ctx.store.lock().unwrap().open_decision_for(attempt.id, "permission", &request)?;
    let decision = match existing {
        Some(d) => d,
        None => ctx.store.lock().unwrap().create_decision(&NewDecision {
            task_id: task.id,
            attempt_id: attempt.id,
            kind: "permission",
            request: &request,
            summary: &summary(tool, tool_input),
            options: &["allow", "deny"],
        })?,
    };
    if decision.status == "pending" {
        let reason = waiting_reason(ctx, task.id)?;
        ctx.store.lock().unwrap().set_task_state(task.id, Some(TaskState::WaitingPermission), &reason)?;
    }
    let Some(answered) = wait(ctx, decision.id, ctx.permission_wait)? else {
        return Ok(None);
    };
    let answer = answered.answer.as_deref().unwrap_or("deny");
    let verb = if answer == "allow" { "allowed" } else { "denied" };
    let reason = format!("owner {verb} {}", answered.summary);
    ctx.store.lock().unwrap().transition_task(task.id, TaskState::WaitingPermission, TaskState::Running, &reason)?;
    Ok(Some(output(answer, answered.note.as_deref())))
}

/// Waits up to `timeout` for the owner to answer; takes the answer if one comes.
fn wait(ctx: &Ctx, id: i64, timeout: Duration) -> Result<Option<Decision>> {
    let deadline = Instant::now() + timeout;
    let mut waiting = ctx.waiters.waiting.lock().unwrap();
    *waiting.entry(id).or_default() += 1;
    let result = loop {
        let taken = (|| -> Result<Option<Decision>> {
            let mut store = ctx.store.lock().unwrap();
            let d = store.decision(id)?;
            Ok((d.status == "answered" && store.apply_decision(id)?).then_some(d))
        })();
        match taken {
            Ok(None) if Instant::now() < deadline => {
                let left = deadline.saturating_duration_since(Instant::now());
                waiting = ctx.waiters.changed.wait_timeout(waiting, left).unwrap().0;
            }
            other => break other,
        }
    };
    if let Some(n) = waiting.get_mut(&id) {
        *n -= 1;
        if *n == 0 {
            waiting.remove(&id);
        }
    }
    result
}

/// How an answer reached, or will reach, the worker.
pub fn decide(ctx: &Ctx, id: i64, answer: &str, note: Option<&str>) -> Result<(Decision, &'static str)> {
    ctx.store.lock().unwrap().answer_decision(id, answer, note)?;
    let waiting = ctx.waiters.waiting.lock().unwrap();
    if waiting.contains_key(&id) {
        ctx.waiters.changed.notify_all();
        drop(waiting);
        return Ok((ctx.store.lock().unwrap().decision(id)?, "delivered to the waiting worker"));
    }
    // No hook is waiting, and none can take the answer while we hold the lock.
    let d = ctx.store.lock().unwrap().decision(id)?;
    drop(waiting);
    if d.status != "answered" || d.kind != "permission" {
        return Ok((d, "recorded"));
    }
    let delivery = deliver_late(ctx, &d)?;
    Ok((ctx.store.lock().unwrap().decision(id)?, delivery))
}

/// Restarts a worker still stopped at Claude's prompt, so the request comes back
/// through the hook (F16). Any other worker gets the answer if it asks again.
fn deliver_late(ctx: &Ctx, d: &Decision) -> Result<&'static str> {
    let Some(attempt_id) = d.attempt_id else { return Ok("recorded") };
    let attempt = ctx.store.lock().unwrap().attempt(attempt_id)?;
    let sessions = ctx.store.lock().unwrap().sessions(attempt_id)?;
    let Some(s) = sessions.iter().find(|s| s.origin == "dispatch") else {
        return Ok("recorded; the worker has no session to deliver it to");
    };
    let session = SessionRef { short_id: s.short_id.clone(), uuid: s.uuid.clone() };
    let observed = ctx.backend.list()?.into_iter().find(|o| o.session.short_id == session.short_id);
    if observed.and_then(|o| o.waiting_for).as_deref() != Some("permission prompt") {
        return Ok("recorded; it applies if the worker asks for exactly this again");
    }
    let task_id = attempt.task_id;
    ctx.store.lock().unwrap().audit_event("decision", d.id, "delivering_by_resume", &json!({ "session": session.short_id }))?;
    ctx.backend.stop(&session)?;
    // Resume only once the process is gone: resuming a live session copies it (F9).
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let running = ctx.backend.list()?.iter().any(|o| o.session.short_id == session.short_id && o.liveness.is_running());
        if !running {
            break;
        }
        if Instant::now() >= deadline {
            bail!("worker {} didn't stop; attach with `claude attach {}` to answer", session.short_id, session.short_id);
        }
        sleep(Duration::from_millis(250));
    }
    match ctx.backend.resume(&session, &attempt.worktree, RESUME_PROMPT)? {
        Resumed::Same(_) => {
            let reason = format!("restarted the worker to deliver decision #{}", d.id);
            ctx.store.lock().unwrap().set_task_state(task_id, Some(TaskState::Running), &reason)?;
            Ok("restarted the worker so it asks again; the answer applies only to the same request")
        }
        Resumed::Copy(copy) => {
            ctx.backend.stop(&copy)?;
            bail!("resuming {} started a copy ({}), which Baton stopped; attach to answer", session.short_id, copy.short_id)
        }
    }
}

/// What an answer binds to: the tool and its exact input, minus the free-text
/// `description` the model rewrites when it retries the same call. serde_json
/// sorts object keys, so equal requests give equal strings.
fn request_key(tool: &str, input: &Value) -> String {
    let mut input = input.clone();
    if let Some(fields) = input.as_object_mut() {
        fields.remove("description");
    }
    json!({ "tool_name": tool, "tool_input": input }).to_string()
}

/// The task's reason while it waits at a permission prompt.
pub fn waiting_reason(ctx: &Ctx, task_id: i64) -> Result<String> {
    let pending = ctx.store.lock().unwrap().pending_decisions(task_id)?;
    Ok(match pending.iter().find(|d| d.kind == "permission") {
        Some(d) => format!("permission #{}: {}; run `baton decide {} allow` or `deny`", d.id, d.summary, d.id),
        None => "waiting at Claude's permission prompt; attach to answer".into(),
    })
}

fn summary(tool: &str, input: &Value) -> String {
    let detail = input["command"].as_str().or(input["file_path"].as_str()).map(str::to_owned).unwrap_or_else(|| {
        let keys: Vec<&str> = input.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        keys.join(", ")
    });
    let mut s = format!("{tool}: {detail}");
    if s.len() > 200 {
        let mut end = 200;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('…');
    }
    s
}

fn output(answer: &str, note: Option<&str>) -> Value {
    let decision = if answer == "allow" {
        json!({ "behavior": "allow" })
    } else {
        json!({ "behavior": "deny", "message": note.unwrap_or("The owner denied this through Baton.") })
    };
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::backend::Liveness;
    use crate::backend::fake::Call;
    use crate::testutil::{Fixture, fixture, start};
    use crate::worker::tick;

    fn bash(command: &str) -> Value {
        json!({ "session_id": "fake0001-x", "agent_type": "baton-worker", "tool_name": "Bash", "tool_input": { "command": command, "description": "x" } })
    }

    /// A dispatched worker that has started as its role.
    fn running() -> (Fixture, i64) {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        f.hook(1, "SessionStart", start("fake0001", Some("baton-worker")));
        (f, t)
    }

    /// Answers the task's first pending decision as soon as it appears.
    fn answer_when_pending(f: &Fixture, t: i64, answer: &str, note: Option<&str>) -> &'static str {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let pending = f.ctx.store.lock().unwrap().pending_decisions(t).unwrap();
            if let Some(d) = pending.first() {
                return decide(&f.ctx, d.id, answer, note).unwrap().1;
            }
            assert!(Instant::now() < deadline, "no decision appeared");
            sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn policy_allows_checks_and_plain_reads_only() {
        let f = fixture();
        let task = f.task(f.add_task("x"));
        let ok = |c: &str| allowed_by_policy(&task, "Bash", &json!({ "command": c }));
        assert_eq!(ok("test -f greeting.txt"), Some("one of the task's checks"));
        for c in ["ls", "cat README.md", "grep -n \"hello world\" src", "git status", "git diff --stat", "wc -l a.txt"] {
            assert_eq!(ok(c), Some("read-only command"), "{c}");
        }
        for c in [
            "rm -rf build",
            "cat a > b",
            "ls | sh",
            "cat /etc/passwd",
            "cat ../secret",
            "cat ~/.ssh/id_rsa",
            "echo $(id)",
            "git push",
            "git log --output=x",
            "git checkout main",
            "python3 -c 'print(1)'",
            "ls *",
            "",
        ] {
            assert_eq!(ok(c), None, "{c}");
        }
        assert_eq!(allowed_by_policy(&task, "Write", &json!({ "file_path": "a" })), None);
    }

    #[test]
    fn policy_requests_are_answered_at_once() {
        let (f, t) = running();
        let out = f.hook(1, "PermissionRequest", bash("git status")).unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert!(f.ctx.store.lock().unwrap().pending_decisions(t).unwrap().is_empty());
    }

    #[test]
    fn the_owner_answers_while_the_hook_waits() {
        let (mut f, t) = running();
        f.ctx.permission_wait = Duration::from_secs(5);
        let (out, delivery) = thread::scope(|s| {
            let hook = s.spawn(|| f.hook(1, "PermissionRequest", bash("python3 -c 'print(1)'")));
            let delivery = answer_when_pending(&f, t, "allow", None);
            (hook.join().unwrap(), delivery)
        });
        assert_eq!(delivery, "delivered to the waiting worker");
        assert_eq!(out.unwrap()["hookSpecificOutput"]["decision"]["behavior"], "allow");
        let task = f.task(t);
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::Running, Some("owner allowed Bash: python3 -c 'print(1)'")));

        // A denial carries the owner's note to the worker.
        let out = thread::scope(|s| {
            let hook = s.spawn(|| f.hook(1, "PermissionRequest", bash("python3 -c 'print(2)'")));
            answer_when_pending(&f, t, "deny", Some("not needed for this task"));
            hook.join().unwrap()
        });
        let decision = &out.unwrap()["hookSpecificOutput"]["decision"];
        assert_eq!((decision["behavior"].as_str(), decision["message"].as_str()), (Some("deny"), Some("not needed for this task")));
        assert!(f.fake.calls().iter().all(|c| !matches!(c, Call::Stop(_))), "nothing was restarted");
    }

    #[test]
    fn a_late_answer_restarts_the_worker_and_applies_to_the_same_request_only() {
        let (f, t) = running();
        let request = bash("python3 -c 'print(3)'");
        assert!(f.hook(1, "PermissionRequest", request.clone()).is_none(), "no answer in time: Claude's prompt stays");
        let task = f.task(t);
        assert_eq!(task.state, TaskState::WaitingPermission);
        assert_eq!(task.state_reason.as_deref(), Some("permission #1: Bash: python3 -c 'print(3)'; run `baton decide 1 allow` or `deny`"));

        f.fake.set_liveness("fake0001", Liveness::Waiting, Some("permission prompt")).unwrap();
        let (d, delivery) = decide(&f.ctx, 1, "allow", None).unwrap();
        assert_eq!(delivery, "restarted the worker so it asks again; the answer applies only to the same request");
        assert_eq!(d.status, "answered");
        let calls = f.fake.calls();
        assert!(
            matches!(&calls[calls.len() - 2..], [Call::Stop(s), Call::Resume { short_id, prompt }] if s == "fake0001" && short_id == "fake0001" && prompt == RESUME_PROMPT),
            "{calls:?}"
        );

        // The resumed worker asks something else first: that needs its own answer.
        assert!(f.hook(1, "PermissionRequest", bash("python3 -c 'print(4)'")).is_none());
        assert_eq!(f.ctx.store.lock().unwrap().pending_decisions(t).unwrap().len(), 1);
        // Then it retries the original request: the late answer applies, once.
        let out = f.hook(1, "PermissionRequest", request.clone()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(f.ctx.store.lock().unwrap().decision(1).unwrap().status, "applied");
        assert!(f.hook(1, "PermissionRequest", request).is_none(), "a third identical request is a new decision");
    }

    #[test]
    fn requests_match_on_the_action_not_its_description() {
        let a = request_key("Bash", &json!({ "command": "make", "description": "Build it" }));
        let b = request_key("Bash", &json!({ "description": "Run the build", "command": "make" }));
        assert_eq!(a, b);
        assert_ne!(a, request_key("Bash", &json!({ "command": "make install" })));
        assert_ne!(a, request_key("Bash", &json!({ "command": "make", "run_in_background": true })));
    }

    #[test]
    fn a_late_answer_waits_when_the_worker_isnt_at_the_prompt() {
        let (f, _) = running();
        assert!(f.hook(1, "PermissionRequest", bash("python3 -c 'print(5)'")).is_none());
        let (_, delivery) = decide(&f.ctx, 1, "deny", None).unwrap();
        assert_eq!(delivery, "recorded; it applies if the worker asks for exactly this again");
        assert!(f.fake.calls().iter().all(|c| !matches!(c, Call::Stop(_))));
    }
}
