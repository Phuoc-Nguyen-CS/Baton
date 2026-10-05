//! The worker lifecycle inside the daemon: dispatch queued tasks, poll liveness,
//! act on hook events. Backend calls block, so callers run these off the event loop.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::backend::claude::{self, PERMISSION_MODE};
use crate::backend::{Backend, DispatchRequest, Liveness, SessionRef};
use crate::git;
use crate::model::{Quota, Task, TaskState};
use crate::paths::Paths;
use crate::permission::{self, Waiters};
use crate::store::{NewAttempt, Next, SessionInfo, Store, now_ms};
use crate::usage;
use crate::verify;

pub struct Ctx {
    pub paths: Paths,
    pub store: Mutex<Store>,
    pub backend: Arc<dyn Backend>,
    /// Recorded with each session: `claude` or `fake`.
    pub backend_name: &'static str,
    /// The `baton` executable the worker's hooks run.
    pub exe: PathBuf,
    pub waiters: Waiters,
    /// How long a permission hook waits for the owner (`permission::WAIT`).
    pub permission_wait: Duration,
    /// Where workers send telemetry; `None` when the receiver isn't running.
    pub otlp_endpoint: Option<String>,
}

impl Ctx {
    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap()
    }
}

pub fn tick(ctx: &Ctx) -> Result<()> {
    dispatch_next(ctx)?;
    verify::verify_pending(ctx)?;
    poll(ctx)
}

/// Starts the next queued task. Each step is recorded before the outside action
/// it covers, so a crash leaves evidence for reconciliation (PLAN §4).
fn dispatch_next(ctx: &Ctx) -> Result<()> {
    // Bound first: a guard in the match scrutinee would live through the arms.
    let next = ctx.store().next_to_dispatch()?;
    let task = match next {
        Next::Dispatch(task) => task,
        Next::Busy { task_id } => {
            let reason = format!("waiting for capacity: task {task_id}'s worker is running (M1 runs one at a time)");
            return ctx.store().note_queued(&reason);
        }
        Next::Idle => return Ok(()),
    };
    let seq = ctx.store().next_attempt_seq(task.id)?;
    let name = format!("baton-{}-{seq}", task.id);
    let worktree = task.repo.join(".claude/worktrees").join(&name);
    let branch = format!("baton/{}", task.id);
    let dir = ctx.paths.attempt_dir(task.id, seq);
    let settings = dir.join("settings.json");
    let role = claude::worker_role();
    let config = json!({
        "backend": ctx.backend_name,
        "name": name,
        "model": task.model,
        "role": role.name,
        "settings": settings,
        "permission_mode": PERMISSION_MODE,
    });
    let (attempt, workspace) = ctx.store().begin_attempt(&NewAttempt {
        task_id: task.id,
        seq,
        worktree: &worktree,
        branch: &branch,
        base_rev: &task.base_rev,
        config: &config,
    })?;

    let prepared = git::ensure_ignored(&task.repo, ".claude/worktrees/", ".claude/worktrees/baton-probe")
        .and_then(|()| git::add_worktree(&task.repo, &worktree, &branch, &task.base_rev));
    if let Err(e) = prepared {
        ctx.store().set_workspace_state(workspace, "failed")?;
        let reason = format!("couldn't create the worktree: {e:#}");
        ctx.store().transition_attempt(attempt, &["preparing"], "failed", Some(TaskState::Failed), &reason)?;
        return Ok(());
    }
    ctx.store().set_workspace_state(workspace, "ready")?;

    let hook_command = format!(
        "BATON_HOME={} {} hook --attempt {attempt} --role {}",
        sh_quote(&ctx.paths.home.to_string_lossy()),
        sh_quote(&ctx.exe.to_string_lossy()),
        role.name
    );
    fs::create_dir_all(&dir)?;
    fs::write(&settings, serde_json::to_vec_pretty(&claude::settings(&hook_command, ctx.otlp_endpoint.as_deref()))?)?;

    ctx.store().transition_attempt(attempt, &["preparing"], "dispatching", None, "starting the worker")?;
    let request = DispatchRequest {
        name: name.clone(),
        cwd: worktree,
        prompt: task_prompt(&task),
        model: task.model.clone(),
        role: Some(role),
        settings: Some(settings),
    };
    match ctx.backend.dispatch(&request) {
        Ok(session) => {
            let info = SessionInfo { short_id: session.short_id, uuid: session.uuid, name: Some(name), origin: "dispatch", ..Default::default() };
            ctx.store().upsert_session(attempt, ctx.backend_name, &info)?;
            ctx.store().transition_attempt(attempt, &["dispatching"], "running", Some(TaskState::Running), "worker started")?;
        }
        Err(e) => {
            let reason = format!("dispatch failed: {e:#}");
            ctx.store().transition_attempt(attempt, &["dispatching"], "failed", Some(TaskState::Failed), &reason)?;
        }
    }
    Ok(())
}

fn task_prompt(task: &Task) -> String {
    let checks = if task.checks.is_empty() {
        "No acceptance checks are defined, so say in your handoff how you verified the result.".to_owned()
    } else {
        let list: Vec<String> = task.checks.iter().map(|c| format!("- `{c}`")).collect();
        format!("When you finish, Baton runs these acceptance checks in this directory:\n{}", list.join("\n"))
    };
    format!("Task: {}\n\n{checks}", task.goal)
}

/// Quotes `s` for a POSIX shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// How long after a turn ends the transcript is re-read for the cross-check.
const TRANSCRIPT_RECHECK: Duration = Duration::from_secs(60);

/// Refreshes liveness from the backend's listing. Only the permission prompt
/// changes task state here; everything else comes from hooks (F19).
fn poll(ctx: &Ctx) -> Result<()> {
    let recent = ctx.store().recent_transcripts(now_ms() - TRANSCRIPT_RECHECK.as_millis() as i64)?;
    for (session, path) in recent {
        if let Ok(tokens) = usage::transcript_tokens(&path) {
            ctx.store().set_transcript_usage(session, &tokens)?;
        }
    }
    let sessions = ctx.store().live_sessions()?;
    if sessions.is_empty() {
        return Ok(());
    }
    let observed = ctx.backend.list()?;
    for s in sessions {
        let obs = observed.iter().find(|o| o.session.short_id == s.short_id);
        let liveness = obs.map_or_else(|| "missing".to_owned(), |o| liveness_name(&o.liveness));
        let waiting_for = obs.and_then(|o| o.waiting_for.as_deref());
        let mut store = ctx.store();
        store.set_liveness(s.id, &liveness, waiting_for)?;
        if let Some(uuid) = obs.and_then(|o| o.session.uuid.clone()).filter(|_| s.uuid.is_none()) {
            let info = SessionInfo { short_id: s.short_id.clone(), uuid: Some(uuid), ..Default::default() };
            store.upsert_session(s.attempt_id, ctx.backend_name, &info)?;
        }
        if s.origin != "dispatch" {
            continue;
        }
        let at_prompt = waiting_for == Some("permission prompt");
        let state = store.task(s.task_id)?.state;
        drop(store);
        if at_prompt && state != TaskState::WaitingPermission {
            let reason = permission::waiting_reason(ctx, s.task_id)?;
            ctx.store().set_task_state(s.task_id, Some(TaskState::WaitingPermission), &reason)?;
        } else if !at_prompt && state == TaskState::WaitingPermission {
            ctx.store().set_task_state(s.task_id, Some(TaskState::Running), "permission prompt answered")?;
        }
    }
    Ok(())
}

fn liveness_name(l: &Liveness) -> String {
    match l {
        Liveness::Busy => "busy".into(),
        Liveness::Waiting => "waiting".into(),
        Liveness::Idle => "idle".into(),
        Liveness::NotRunning => "not_running".into(),
        Liveness::Unknown(s) => format!("unknown:{s}"),
    }
}

/// Acts on one hook event from attempt `attempt_id`; returns hook output, if any.
pub fn on_hook(ctx: &Ctx, attempt_id: i64, event: &str, input: &Value, denied: Option<&str>) -> Result<Option<Value>> {
    let attempt = ctx.store().attempt(attempt_id)?;
    let session_id = input["session_id"].as_str().context("hook input has no session_id")?;
    let short_id = session_id.get(..8).context("session_id is too short")?.to_owned();

    // Before dispatch returns, the first session seen is the dispatched one; any
    // other id is a copy, which can write into this worktree (F10).
    let known = ctx.store().sessions(attempt_id)?;
    let origin = match known.iter().find(|s| s.short_id == short_id) {
        Some(s) if s.origin == "dispatch" => "dispatch",
        Some(_) => "copy",
        None if attempt.state == "dispatching" && !known.iter().any(|s| s.origin == "dispatch") => "dispatch",
        None => "copy",
    };
    let text = |k: &str| input[k].as_str().map(str::to_owned);
    let info = SessionInfo {
        short_id: short_id.clone(),
        uuid: Some(session_id.to_owned()),
        origin,
        agent_type: text("agent_type"),
        model: text("model"),
        transcript: text("transcript_path"),
        ..Default::default()
    };
    let (session_row, new) = ctx.store().upsert_session(attempt_id, ctx.backend_name, &info)?;
    let session = SessionRef { short_id: short_id.clone(), uuid: Some(session_id.to_owned()) };

    if origin == "copy" {
        if new {
            ctx.backend.stop(&session)?;
            ctx.store().set_task_state(attempt.task_id, None, &format!("stopped an unexpected copy of the worker ({short_id})"))?;
        }
        return Ok(None);
    }

    let activity = |reason: &str| ctx.store().note_activity(attempt.task_id, reason);
    match event {
        "SessionStart" => {
            let expected: Value = serde_json::from_str(&attempt.config)?;
            let expected = expected["role"].as_str().unwrap_or_default();
            let reported = input["agent_type"].as_str();
            if reported != Some(expected) {
                // A missing role means default tools: stop rather than run wider (F6).
                ctx.backend.stop(&session)?;
                let reason = format!(
                    "role mismatch: the worker started as {}, not {expected}; stopped it",
                    reported.unwrap_or("no agent")
                );
                ctx.store().transition_attempt(attempt_id, &["dispatching", "running", "turn_ended"], "failed", Some(TaskState::Failed), &reason)?;
            } else {
                let source = input["source"].as_str().unwrap_or("startup");
                activity(&format!("worker session started ({source})"))?;
            }
        }
        "UserPromptSubmit" => {
            // A new turn: the previous candidate is no longer what the worker is producing.
            let reason = "worker received its instructions";
            if !ctx.store().transition_attempt(attempt_id, &["turn_ended"], "running", Some(TaskState::Running), reason)? {
                activity(reason)?;
            }
        }
        "PreToolUse" => {
            let tool = input["tool_name"].as_str().unwrap_or("a tool");
            match denied {
                Some(reason) => {
                    ctx.store().audit_event("attempt", attempt_id, "guard_denied", &json!({ "tool": tool }))?;
                    activity(&format!("Baton blocked {tool}: {reason}"))?;
                }
                None => activity(&format!("using {tool}"))?,
            }
        }
        "PermissionRequest" => {
            let task = ctx.store().task(attempt.task_id)?;
            return permission::on_request(ctx, &task, &attempt, input);
        }
        "Notification" => match input["notification_type"].as_str() {
            Some("permission_prompt") => {
                // Arrives ~6 s into a request, possibly while Baton's hook still waits (F16).
                let reason = permission::waiting_reason(ctx, attempt.task_id)?;
                ctx.store().set_task_state(attempt.task_id, Some(TaskState::WaitingPermission), &reason)?;
            }
            Some("idle_prompt") => activity("worker is idle")?,
            _ => {}
        },
        "Stop" => {
            let message = input["last_assistant_message"].as_str().unwrap_or_default();
            let dir = ctx.paths.attempt_dir(attempt.task_id, attempt.seq);
            fs::create_dir_all(&dir)?;
            fs::write(dir.join("last-message.txt"), message)?;
            // A blocked worker waits for the owner; anything else gets verified,
            // whatever the worker claims (F15).
            let (state, reason) = match handoff_field(message, "STATUS") {
                Some(s) if s.eq_ignore_ascii_case("blocked") => {
                    let why = handoff_field(message, "OPEN").or(handoff_field(message, "SUMMARY")).unwrap_or("no reason given");
                    (TaskState::WaitingInput, format!("worker is blocked: {why}"))
                }
                Some(_) => (TaskState::Verifying, "worker finished its turn; running checks".to_owned()),
                None => (TaskState::Verifying, "worker finished without a handoff; running checks".to_owned()),
            };
            ctx.store().transition_attempt(attempt_id, &["dispatching", "running"], "turn_ended", Some(state), &reason)?;
            // Cross-check usage against the transcript; if it can't be read, the
            // cross-check stays unknown.
            let path = ctx.store().transcript_path(session_row)?;
            if let Some(tokens) = path.and_then(|p| usage::transcript_tokens(&p).ok()) {
                ctx.store().set_transcript_usage(session_row, &tokens)?;
            }
        }
        "StatusLine" => {
            let limits = &input["rate_limits"];
            if limits.is_object() {
                let quota = Quota {
                    five_hour_pct: limits["five_hour"]["used_percentage"].as_f64(),
                    five_hour_resets_at: limits["five_hour"]["resets_at"].as_i64(),
                    seven_day_pct: limits["seven_day"]["used_percentage"].as_f64(),
                    seven_day_resets_at: limits["seven_day"]["resets_at"].as_i64(),
                    observed_ms: now_ms(),
                };
                ctx.store().set_quota(&quota)?;
            }
        }
        "SessionEnd" => {
            ctx.store().set_liveness(session_row, "not_running", None)?;
            let reason = input["reason"].as_str().unwrap_or("unknown");
            activity(&format!("worker session ended ({reason})"))?;
        }
        _ => {}
    }
    Ok(None)
}

/// The value of a `FIELD: value` line in the worker's handoff, if present.
fn handoff_field<'a>(message: &'a str, field: &str) -> Option<&'a str> {
    message.lines().rev().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        (key.trim() == field).then(|| value.trim()).filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::Call;
    use crate::git::tests::git_in;
    use crate::testutil::{fixture, start};

    #[test]
    fn dispatch_prepares_the_worktree_settings_and_session() {
        let f = fixture();
        let t1 = f.add_task("add a greeting");
        let t2 = f.add_task("second task");
        tick(&f.ctx).unwrap();

        let task = f.task(t1);
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::Running, Some("worker started")));
        let w = f.worker(t1);
        assert_eq!(w.attempt_state, "running");
        assert_eq!(w.session.as_deref(), Some("fake0001"));
        assert_eq!(w.worktree, f.repo.join(".claude/worktrees/baton-1-1"));
        assert_eq!(git::repo(&w.worktree).unwrap().head, f.head, "the worktree starts at the task's base");
        let exclude = fs::read_to_string(f.repo.join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|l| l == ".claude/worktrees/"));

        let settings: Value = serde_json::from_slice(&fs::read(f.ctx.paths.attempt_dir(t1, 1).join("settings.json")).unwrap()).unwrap();
        let command = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        let home = f.ctx.paths.home.display();
        assert_eq!(command, format!("BATON_HOME='{home}' '/opt/baton/bin/baton' hook --attempt 1 --role baton-worker Stop"));

        let calls = f.fake.calls();
        let [Call::Dispatch(req)] = calls.as_slice() else { panic!("{calls:?}") };
        assert_eq!((req.name.as_str(), req.cwd.as_path()), ("baton-1-1", w.worktree.as_path()));
        assert!(req.prompt.starts_with("Task: add a greeting") && req.prompt.contains("- `test -f greeting.txt`"));
        assert_eq!(req.role.as_ref().unwrap().name, "baton-worker");
        assert_eq!(req.model.as_deref(), Some("haiku"));

        // One worker at a time: the second task waits, and says why.
        tick(&f.ctx).unwrap();
        let waiting = f.task(t2);
        assert_eq!(waiting.state, TaskState::Queued);
        assert_eq!(waiting.state_reason.as_deref(), Some("waiting for capacity: task 1's worker is running (M1 runs one at a time)"));
        assert_eq!(f.fake.calls().len(), 1);

        // Once the first worker's turn ends, the second task starts.
        f.hook(1, "Stop", json!({ "session_id": "fake0001-x", "last_assistant_message": "STATUS: blocked\nOPEN: which file?" }));
        tick(&f.ctx).unwrap();
        assert_eq!(f.task(t2).state, TaskState::Running);
        assert_eq!(f.fake.calls().len(), 2);
    }

    #[test]
    fn hooks_drive_the_task_through_a_turn() {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();

        f.hook(1, "SessionStart", start("fake0001", Some("baton-worker")));
        assert_eq!(f.worker(t).agent_type.as_deref(), Some("baton-worker"));
        assert_eq!(f.task(t).state_reason.as_deref(), Some("worker session started (startup)"));

        f.hook(1, "PreToolUse", json!({ "session_id": "fake0001-x", "tool_name": "Write" }));
        assert_eq!(f.task(t).state_reason.as_deref(), Some("using Write"));
        let input = json!({ "session_id": "fake0001-x", "tool_name": "Bash", "hook_event_name": "PreToolUse" });
        on_hook(&f.ctx, 1, "PreToolUse", &input, Some("no pushing")).unwrap();
        assert_eq!(f.task(t).state_reason.as_deref(), Some("Baton blocked Bash: no pushing"));

        f.hook(1, "Notification", json!({ "session_id": "fake0001-x", "notification_type": "permission_prompt" }));
        assert_eq!(f.task(t).state, TaskState::WaitingPermission);

        f.hook(1, "Stop", json!({ "session_id": "fake0001-x", "last_assistant_message": "STATUS: done" }));
        assert_eq!(f.worker(t).attempt_state, "turn_ended");
        let message = fs::read_to_string(f.ctx.paths.attempt_dir(t, 1).join("last-message.txt")).unwrap();
        assert_eq!(message, "STATUS: done");

        f.hook(1, "UserPromptSubmit", json!({ "session_id": "fake0001-x" }));
        assert_eq!(f.worker(t).attempt_state, "running");

        let denied: i64 = f
            .ctx
            .store()
            .conn()
            .query_row("SELECT count(*) FROM audit WHERE event = 'guard_denied'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(denied, 1);
    }

    #[test]
    fn a_finished_turn_is_snapshotted_and_checked() {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        f.hook(1, "SessionStart", start("fake0001", Some("baton-worker")));
        let worktree = f.worker(t).worktree;
        fs::write(worktree.join("greeting.txt"), "Hello\n").unwrap();
        let handoff = "Created it.\nSTATUS: done\nSUMMARY: added greeting.txt\nCHANGED: greeting.txt\nCHECKS: none\nOPEN: none";
        f.hook(1, "Stop", json!({ "session_id": "fake0001-x", "last_assistant_message": handoff }));
        assert_eq!(f.task(t).state, TaskState::Verifying);

        tick(&f.ctx).unwrap();
        let task = f.task(t);
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::ReviewReady, Some("ready for review: 1 of 1 checks passed")));
        let view = f.ctx.store().task_views().unwrap().pop().unwrap();
        assert!(git::is_at(&worktree, &view.candidate.unwrap().commit).unwrap());

        // Activity after the turn doesn't overwrite the review summary.
        f.hook(1, "SessionEnd", json!({ "session_id": "fake0001-x", "reason": "other" }));
        assert_eq!(f.task(t).state_reason.as_deref(), Some("ready for review: 1 of 1 checks passed"));
        assert_eq!(f.worker(t).liveness.as_deref(), Some("not_running"));

        // The worker gets new instructions: the task is running again.
        f.hook(1, "UserPromptSubmit", json!({ "session_id": "fake0001-x" }));
        assert_eq!(f.task(t).state, TaskState::Running);
    }

    #[test]
    fn the_transcript_is_read_again_after_stop() {
        // Seen in smoke run 3: at Stop the transcript lacked the last message's usage.
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        let transcript = f.ctx.paths.home.join("t.jsonl");
        let entry = |id: &str| json!({ "type": "assistant", "message": { "id": id, "usage": { "input_tokens": 1, "output_tokens": 1 } } }).to_string();
        fs::write(&transcript, entry("m1")).unwrap();
        let input = json!({ "session_id": "fake0001-x", "transcript_path": transcript, "last_assistant_message": "STATUS: done" });
        f.hook(1, "Stop", input);
        let stored = || f.ctx.store().conn().query_row("SELECT transcript_usage FROM session", [], |r| r.get::<_, String>(0)).unwrap();
        assert!(stored().contains("\"input\":1"));

        fs::write(&transcript, format!("{}\n{}", entry("m1"), entry("m2"))).unwrap();
        tick(&f.ctx).unwrap();
        assert!(stored().contains("\"input\":2"), "{}", stored());
        assert_eq!(f.task(t).state, TaskState::ReviewReady);
    }

    #[test]
    fn a_blocked_handoff_waits_for_the_owner() {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        let handoff = "STATUS: blocked\nSUMMARY: need a decision\nOPEN: which greeting language?";
        f.hook(1, "Stop", json!({ "session_id": "fake0001-x", "last_assistant_message": handoff }));
        tick(&f.ctx).unwrap();
        let task = f.task(t);
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::WaitingInput, Some("worker is blocked: which greeting language?")));
        assert!(f.ctx.store().latest_candidate(t).unwrap().is_none(), "nothing to verify yet");
    }

    #[test]
    fn handoff_fields_come_from_the_last_matching_line() {
        let m = "STATUS: draft\nwork…\nSTATUS: done\nOPEN:\n";
        assert_eq!(handoff_field(m, "STATUS"), Some("done"));
        assert_eq!(handoff_field(m, "OPEN"), None);
        assert_eq!(handoff_field("no handoff here", "STATUS"), None);
    }

    #[test]
    fn a_worker_without_its_role_is_stopped_and_fails() {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        f.hook(1, "SessionStart", start("fake0001", None));

        let task = f.task(t);
        assert_eq!(task.state, TaskState::Failed);
        assert!(task.state_reason.unwrap().starts_with("role mismatch: the worker started as no agent"));
        assert!(f.fake.calls().contains(&Call::Stop("fake0001".into())));
        // Later events can't revive it.
        f.hook(1, "Stop", json!({ "session_id": "fake0001-x" }));
        assert_eq!(f.task(t).state, TaskState::Failed);
    }

    #[test]
    fn an_unexpected_session_is_a_copy_and_gets_stopped() {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        f.hook(1, "SessionStart", start("fake0001", Some("baton-worker")));

        // Resuming a live session copies it (F9); its hooks carry the new id.
        let original = SessionRef { short_id: "fake0001".into(), uuid: None };
        let crate::backend::Resumed::Copy(copy) = f.fake.resume(&original, &f.repo, "continue").unwrap() else {
            panic!("live resume must copy");
        };
        f.hook(1, "SessionStart", start(&copy.short_id, Some("baton-worker")));

        assert!(f.fake.calls().contains(&Call::Stop(copy.short_id.clone())));
        let task = f.task(t);
        assert_eq!(task.state, TaskState::Running);
        assert_eq!(task.state_reason, Some(format!("stopped an unexpected copy of the worker ({})", copy.short_id)));
        assert_eq!(f.worker(t).session.as_deref(), Some("fake0001"), "status still shows the dispatched session");
    }

    #[test]
    fn worktree_and_dispatch_failures_fail_the_task() {
        let f = fixture();
        git_in(&f.repo, &["branch", "baton/1"]);
        let t1 = f.add_task("branch taken");
        tick(&f.ctx).unwrap();
        let task = f.task(t1);
        assert_eq!(task.state, TaskState::Failed);
        assert!(task.state_reason.unwrap().starts_with("couldn't create the worktree"));
        assert!(f.fake.calls().is_empty());

        f.fake.fail_next_dispatch("Workspace not trusted");
        let t2 = f.add_task("untrusted");
        tick(&f.ctx).unwrap();
        let task = f.task(t2);
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::Failed, Some("dispatch failed: Workspace not trusted")));
        assert_eq!(f.worker(t2).attempt_state, "failed");
    }

    #[test]
    fn polling_tracks_the_permission_prompt() {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        f.fake.set_liveness("fake0001", Liveness::Waiting, Some("permission prompt")).unwrap();
        tick(&f.ctx).unwrap();
        assert_eq!(f.task(t).state, TaskState::WaitingPermission);
        assert_eq!(f.worker(t).liveness.as_deref(), Some("waiting"));

        f.fake.set_liveness("fake0001", Liveness::Busy, None).unwrap();
        tick(&f.ctx).unwrap();
        let task = f.task(t);
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::Running, Some("permission prompt answered")));
    }
}
