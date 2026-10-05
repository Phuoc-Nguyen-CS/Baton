//! The owner's review (PLAN §6): the evidence behind a candidate, and verdicts
//! bound to the exact candidate the owner saw.

use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::backend::SessionRef;
use crate::git;
use crate::model::{Task, TaskDetail, TaskState};
use crate::protocol::Verdict;
use crate::store::NewDecision;
use crate::worker::Ctx;

const MAX_PATCH: usize = 256 << 10;
const MAX_MESSAGE: usize = 64 << 10;

pub fn detail(ctx: &Ctx, task_id: i64) -> Result<TaskDetail> {
    let (task, candidate, attempt) = {
        let store = ctx.store.lock().unwrap();
        (store.task(task_id)?, store.latest_candidate(task_id)?, store.latest_attempt(task_id)?)
    };
    let last_message = attempt
        .and_then(|a| fs::read_to_string(ctx.paths.attempt_dir(task_id, a.seq).join("last-message.txt")).ok())
        .map(|m| cut(m, MAX_MESSAGE).0);
    let Some(c) = candidate else {
        return Ok(TaskDetail { last_message, ..Default::default() });
    };
    let range = format!("{}..{}", task.base_rev, c.commit);
    let git = |args: &[&str]| git::output(Command::new("git").arg("-C").arg(&task.repo).args(args));
    let stat = git(&["diff", "--stat", &range])?;
    let (patch, truncated) = cut(git(&["diff", &range])?, MAX_PATCH);
    Ok(TaskDetail { stat, patch, truncated, last_message })
}

fn cut(mut s: String, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    (s, true)
}

/// Applies the owner's verdict; returns the task and what happened.
pub fn review(ctx: &Ctx, task_id: i64, verdict: Verdict, seen: Option<&str>, note: Option<&str>) -> Result<(Task, String)> {
    let store = || ctx.store.lock().unwrap();
    let task = store().task(task_id)?;
    if task.state != TaskState::ReviewReady {
        bail!("task {task_id} is {}, not ready for review", task.state.as_str());
    }
    let candidate = store().latest_candidate(task_id)?.context("no candidate to review")?;
    let short = &candidate.commit[..12];
    if let Some(seen) = seen
        && (seen.len() < 7 || !candidate.commit.starts_with(seen))
    {
        bail!("the candidate changed: you reviewed {seen}, the current one is {short}; review it again");
    }
    let attempt = store().latest_attempt(task_id)?.context("no attempt")?;
    let answer = match verdict {
        Verdict::Accept => "accept",
        Verdict::Defer => "defer",
        Verdict::Changes => bail!("requesting changes arrives in M1.8; for now attach to the worker with `claude attach`"),
    };
    if verdict == Verdict::Accept && !git::is_at(&attempt.worktree, &candidate.commit)? {
        bail!("the worktree no longer matches candidate {short}; wait for Baton to verify the new state");
    }

    // Every verdict is a recorded human decision (PLAN §11, M1).
    let request = json!({ "candidate": candidate.commit }).to_string();
    let decision = store().create_decision(&NewDecision {
        task_id,
        attempt_id: attempt.id,
        kind: "review",
        request: &request,
        summary: &format!("review candidate {short}"),
        options: &["accept", "changes", "defer"],
    })?;
    store().answer_decision(decision.id, answer, note)?;
    store().apply_decision(decision.id)?;

    let outcome = match verdict {
        Verdict::Accept => {
            let reason = format!("accepted candidate {short}; it's on {} for you to merge", attempt.branch);
            store().transition_task(task_id, TaskState::ReviewReady, TaskState::Accepted, &reason)?;
            // The worker is done: stop its idle process. The worktree and branch stay.
            let sessions = store().sessions(attempt.id)?;
            for s in sessions.iter().filter(|s| s.liveness.as_deref().is_some_and(|l| matches!(l, "busy" | "waiting" | "idle"))) {
                let session = SessionRef { short_id: s.short_id.clone(), uuid: s.uuid.clone() };
                if let Err(e) = ctx.backend.stop(&session) {
                    eprintln!("baton daemon: stopping {} after accept: {e:#}", s.short_id);
                }
            }
            format!("{reason} (`git merge {}`)", attempt.branch)
        }
        _ => {
            store().set_task_state(task_id, None, "deferred by the owner")?;
            "deferred; the task stays ready for review".to_owned()
        }
    };
    Ok((store().task(task_id)?, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::Call;
    use crate::testutil::{fixture, start};
    use crate::worker::tick;
    use serde_json::json;

    /// A task whose worker wrote greeting.txt and whose check passed.
    fn reviewable() -> (crate::testutil::Fixture, i64) {
        let f = fixture();
        let t = f.add_task("add a greeting");
        tick(&f.ctx).unwrap();
        f.hook(1, "SessionStart", start("fake0001", Some("baton-worker")));
        fs::write(f.worker(t).worktree.join("greeting.txt"), "Hello\n").unwrap();
        f.hook(1, "Stop", json!({ "session_id": "fake0001-x", "last_assistant_message": "STATUS: done\nSUMMARY: added it" }));
        tick(&f.ctx).unwrap();
        assert_eq!(f.task(t).state, TaskState::ReviewReady);
        (f, t)
    }

    #[test]
    fn detail_shows_the_candidates_diff_and_the_handoff() {
        let (f, t) = reviewable();
        let d = detail(&f.ctx, t).unwrap();
        assert!(d.stat.contains("greeting.txt | 1 +"), "{}", d.stat);
        assert!(d.patch.contains("+Hello"));
        assert!(!d.truncated);
        assert_eq!(d.last_message.as_deref(), Some("STATUS: done\nSUMMARY: added it"));
    }

    #[test]
    fn accept_is_bound_to_the_candidate_the_owner_saw() {
        let (f, t) = reviewable();
        let commit = f.ctx.store.lock().unwrap().latest_candidate(t).unwrap().unwrap().commit;

        let err = review(&f.ctx, t, Verdict::Accept, Some("0000000"), None).unwrap_err();
        assert!(err.to_string().contains("the candidate changed"), "{err}");
        // An edit after verification makes the candidate stale too.
        let wt = f.worker(t).worktree;
        fs::write(wt.join("greeting.txt"), "Changed\n").unwrap();
        assert!(review(&f.ctx, t, Verdict::Accept, Some(&commit[..12]), None).unwrap_err().to_string().contains("no longer matches"));
        fs::write(wt.join("greeting.txt"), "Hello\n").unwrap();

        let (task, outcome) = review(&f.ctx, t, Verdict::Accept, Some(&commit[..12]), None).unwrap();
        assert_eq!(task.state, TaskState::Accepted);
        assert!(outcome.contains("baton/1 for you to merge"), "{outcome}");
        assert!(f.fake.calls().contains(&Call::Stop("fake0001".into())), "the idle worker is stopped");
        let d = f.ctx.store.lock().unwrap().decision(1).unwrap();
        assert_eq!((d.kind.as_str(), d.status.as_str(), d.answer.as_deref()), ("review", "applied", Some("accept")));
        assert!(review(&f.ctx, t, Verdict::Accept, None, None).unwrap_err().to_string().contains("not ready for review"));
    }

    #[test]
    fn defer_keeps_the_task_ready() {
        let (f, t) = reviewable();
        let (task, _) = review(&f.ctx, t, Verdict::Defer, None, Some("after lunch")).unwrap();
        assert_eq!((task.state, task.state_reason.as_deref()), (TaskState::ReviewReady, Some("deferred by the owner")));
        assert_eq!(f.ctx.store.lock().unwrap().decision(1).unwrap().note.as_deref(), Some("after lunch"));
    }
}
