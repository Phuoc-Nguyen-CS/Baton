//! Baton's own verification (PLAN §6): snapshot the worker's result as an exact
//! candidate, run the task's checks on it, and tie every result to that candidate.
//! The worker's report is never evidence (F15).

use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

use crate::git;
use crate::model::{Task, TaskState};
use crate::store::Attempt;
use crate::worker::Ctx;

const CHECK_TIMEOUT: Duration = Duration::from_secs(600);

pub fn verify_pending(ctx: &Ctx) -> Result<()> {
    let pending = ctx.store.lock().unwrap().tasks_to_verify()?;
    for (task, attempt) in pending {
        verify(ctx, &task, &attempt, CHECK_TIMEOUT)?;
    }
    Ok(())
}

fn verify(ctx: &Ctx, task: &Task, attempt: &Attempt, timeout: Duration) -> Result<()> {
    let store = || ctx.store.lock().unwrap();
    let message = format!("baton: candidate for task {} (attempt {})\n\n{}", task.id, attempt.seq, task.goal);
    let (commit, tree) = match git::snapshot(&attempt.worktree, &message) {
        Ok(snapshot) => snapshot,
        Err(e) => {
            let reason = format!("couldn't snapshot the worktree: {e:#}");
            store().transition_task(task.id, TaskState::Verifying, TaskState::Failed, &reason)?;
            return Ok(());
        }
    };
    let candidate = store().add_candidate(attempt, &commit, &tree, &task.base_rev)?;
    store().interrupt_checks(candidate)?;

    let dir = ctx.paths.attempt_dir(task.id, attempt.seq).join("checks");
    fs::create_dir_all(&dir)?;
    let env = json!({ "baton": env!("CARGO_PKG_VERSION"), "shell": "sh", "cwd": attempt.worktree });
    let (mut passed, mut errors) = (0, 0);
    for check in &task.checks {
        let id = store().start_check(candidate, check, &env)?;
        let log = dir.join(format!("{id}.log"));
        let (state, code) = match run_check(check, &attempt.worktree, &log, timeout) {
            Ok(Outcome::Exited(Some(0))) => ("passed", Some(0)),
            Ok(Outcome::Exited(Some(code))) => ("failed", Some(code)),
            Ok(Outcome::Exited(None)) => note(&log, "killed by a signal"),
            Ok(Outcome::TimedOut) => note(&log, &format!("timed out after {} s", timeout.as_secs())),
            Err(e) => note(&log, &format!("couldn't run: {e:#}")),
        };
        passed += usize::from(state == "passed");
        errors += usize::from(state == "error");
        store().finish_check(id, state, code, &log)?;
    }

    // Results count only if the worktree still holds exactly the candidate.
    let reason = if !git::is_at(&attempt.worktree, &commit).unwrap_or(false) {
        store().invalidate_checks(candidate)?;
        "the worktree changed during the checks, so their results don't apply to the candidate".to_owned()
    } else if task.checks.is_empty() {
        "ready for review: no checks defined".to_owned()
    } else {
        let mut reason = format!("ready for review: {passed} of {} checks passed", task.checks.len());
        if errors > 0 {
            reason.push_str(&format!(", {errors} couldn't run"));
        }
        reason
    };
    store().transition_task(task.id, TaskState::Verifying, TaskState::ReviewReady, &reason)?;
    Ok(())
}

/// Appends Baton's own note to a check's output; the check counts as an error.
fn note(log: &Path, text: &str) -> (&'static str, Option<i32>) {
    let mut existing = fs::read_to_string(log).unwrap_or_default();
    existing.push_str(&format!("\n[baton] {text}\n"));
    let _ = fs::write(log, existing);
    ("error", None)
}

enum Outcome {
    /// Exit code; `None` when killed by a signal.
    Exited(Option<i32>),
    TimedOut,
}

/// Runs `command` with `sh -c` in `cwd`, output to `log`, in its own process
/// group so a timeout kills everything it started.
fn run_check(command: &str, cwd: &Path, log: &Path, timeout: Duration) -> Result<Outcome> {
    let out = File::create(log)?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .process_group(0)
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Outcome::Exited(status.code()));
        }
        if Instant::now() >= deadline {
            let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
            child.wait()?;
            return Ok(Outcome::TimedOut);
        }
        sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::git::tests::git_in;
    use crate::paths::Paths;
    use crate::store::{NewAttempt, NewTask, Store};

    struct Fixture {
        ctx: Ctx,
        repo: PathBuf,
        _dirs: (tempfile::TempDir, tempfile::TempDir),
    }

    /// A task in `verifying` whose worker left `greeting.txt` in its worktree.
    fn finished_task(checks: &[&str]) -> (Fixture, Task, Attempt) {
        let (home, repos) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let repo = repos.path().join("r");
        fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let head = git::repo(&repo).unwrap().head;
        let ctx = Ctx {
            paths: Paths { home: home.path().into() },
            store: Mutex::new(Store::open_in_memory().unwrap()),
            backend: Arc::new(FakeBackend::new()),
            backend_name: "fake",
            exe: "/bin/baton".into(),
            waiters: Default::default(),
            permission_wait: Duration::from_millis(200),
        };
        let worktree = repo.join(".claude/worktrees/baton-1-1");
        git::add_worktree(&repo, &worktree, "baton/1", &head).unwrap();
        fs::write(worktree.join("greeting.txt"), "Hello\n").unwrap();

        let mut store = ctx.store.lock().unwrap();
        let new = NewTask {
            request_id: "r".into(),
            repo: repo.clone(),
            base_rev: head.clone(),
            goal: "add a greeting".into(),
            checks: checks.iter().map(|c| c.to_string()).collect(),
            model: None,
        };
        let task = store.create_task(&new).unwrap().0;
        let config = json!({});
        let (attempt, _) = store
            .begin_attempt(&NewAttempt { task_id: task.id, seq: 1, worktree: &worktree, branch: "baton/1", base_rev: &head, config: &config })
            .unwrap();
        store.transition_attempt(attempt, &["preparing"], "turn_ended", Some(TaskState::Verifying), "worker finished").unwrap();
        let (task, attempt) = store.tasks_to_verify().unwrap().pop().unwrap();
        drop(store);
        (Fixture { ctx, repo, _dirs: (home, repos) }, task, attempt)
    }

    fn view(f: &Fixture) -> crate::model::TaskView {
        f.ctx.store.lock().unwrap().task_views().unwrap().pop().unwrap()
    }

    #[test]
    fn passing_and_failing_checks_are_tied_to_the_candidate() {
        let (f, _, attempt) = finished_task(&["grep -q Hello greeting.txt", "test -f missing.txt"]);
        verify_pending(&f.ctx).unwrap();

        let v = view(&f);
        assert_eq!(v.task.state, TaskState::ReviewReady);
        assert_eq!(v.task.state_reason.as_deref(), Some("ready for review: 1 of 2 checks passed"));
        let c = v.candidate.unwrap();
        assert!(git::is_at(&attempt.worktree, &c.commit).unwrap(), "the candidate is exactly the worktree");
        let branch_head = git::output(Command::new("git").arg("-C").arg(&f.repo).args(["rev-parse", "baton/1"])).unwrap();
        assert_eq!(branch_head, c.commit, "the candidate is on baton/<task>");
        let states: Vec<_> = c.checks.iter().map(|r| (r.state.as_str(), r.exit_code)).collect();
        assert_eq!(states, [("passed", Some(0)), ("failed", Some(1))]);
        assert!(c.checks[0].output.as_ref().unwrap().exists());

        // Nothing more to do until the worker finishes another turn.
        verify_pending(&f.ctx).unwrap();
        assert_eq!(view(&f).candidate.unwrap().id, c.id);
    }

    #[test]
    fn a_check_that_edits_the_worktree_invalidates_the_results() {
        let (f, _, _) = finished_task(&["echo more >> greeting.txt"]);
        verify_pending(&f.ctx).unwrap();
        let v = view(&f);
        assert_eq!(v.task.state, TaskState::ReviewReady);
        assert!(v.task.state_reason.unwrap().starts_with("the worktree changed during the checks"));
        assert_eq!(v.candidate.unwrap().checks[0].state, "invalidated");
    }

    #[test]
    fn timeouts_and_restarts_are_errors_not_passes() {
        let (f, task, attempt) = finished_task(&["sleep 30 & sleep 30"]);
        let started = Instant::now();
        verify(&f.ctx, &task, &attempt, Duration::from_millis(300)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "the whole process group was killed");
        let v = view(&f);
        assert_eq!(v.task.state_reason.as_deref(), Some("ready for review: 0 of 1 checks passed, 1 couldn't run"));
        let check = &v.candidate.unwrap().checks[0];
        assert_eq!(check.state, "error");
        assert!(fs::read_to_string(check.output.as_ref().unwrap()).unwrap().contains("[baton] timed out"));
    }

    #[test]
    fn no_checks_is_said_plainly() {
        let (f, _, _) = finished_task(&[]);
        verify_pending(&f.ctx).unwrap();
        assert_eq!(view(&f).task.state_reason.as_deref(), Some("ready for review: no checks defined"));
    }

    #[test]
    fn a_verification_cut_short_reuses_the_candidate() {
        let (f, task, attempt) = finished_task(&["true"]);
        // A daemon died mid-check: the candidate exists with a check still running.
        let (commit, tree) = git::snapshot(&attempt.worktree, "baton: candidate").unwrap();
        let candidate = f.ctx.store.lock().unwrap().add_candidate(&attempt, &commit, &tree, &task.base_rev).unwrap();
        f.ctx.store.lock().unwrap().start_check(candidate, "true", &json!({})).unwrap();

        verify_pending(&f.ctx).unwrap();
        let c = view(&f).candidate.unwrap();
        assert_eq!(c.id, candidate);
        assert_eq!(c.checks[0].state, "passed");
        let interrupted: i64 = f
            .ctx
            .store
            .lock()
            .unwrap()
            .conn()
            .query_row("SELECT count(*) FROM verification WHERE state = 'error'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(interrupted, 1, "the interrupted run is kept as an error");
    }
}
