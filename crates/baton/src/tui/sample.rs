//! A fixed state for TUI tests: one task ready for review, one waiting on a
//! permission, one accepted.

use std::path::PathBuf;

use super::app::App;
use crate::model::{Candidate, CheckResult, Decision, Quota, Task, TaskDetail, TaskState, TaskView, Tokens, Usage, Worker};

pub const NOW: i64 = 1_791_160_000_000;

fn task(id: i64, state: TaskState, reason: &str, goal: &str) -> Task {
    Task {
        id,
        request_id: format!("r{id}"),
        repo: PathBuf::from("/home/me/sandbox"),
        base_rev: "9686e63aad021f251e9be3a806495c79d0e1492a".into(),
        goal: goal.into(),
        checks: vec!["grep -qx ok usage.txt".into()],
        model: Some("haiku".into()),
        state,
        state_reason: Some(reason.into()),
        observed_ms: NOW,
        created_ms: NOW - 60_000,
    }
}

fn worker(attempt: i64, state: &str, session: &str, liveness: &str, task: i64) -> Worker {
    Worker {
        attempt,
        attempt_state: state.into(),
        worktree: PathBuf::from(format!("/home/me/sandbox/.claude/worktrees/baton-{task}-1")),
        branch: format!("baton/{task}"),
        session: Some(session.into()),
        backend: Some("claude".into()),
        liveness: Some(liveness.into()),
        waiting_for: None,
        agent_type: Some("baton-worker".into()),
    }
}

pub fn app() -> App {
    let tokens = Tokens { input: 18, output: 422, cache_read: 11627, cache_write: 2851 };
    let review = TaskView {
        task: task(3, TaskState::ReviewReady, "ready for review: 1 of 1 checks passed", "Create a file named usage.txt containing the single word ok."),
        worker: Some(worker(3, "turn_ended", "411afd47", "idle", 3)),
        candidate: Some(Candidate {
            id: 3,
            commit: "206e9624e4d8a1b2c3d4e5f60718293a4b5c6d7e".into(),
            tree: "a46d6a0a8c7e2db40a2ac8c65a4710d00194a952".into(),
            checks: vec![CheckResult { command: "grep -qx ok usage.txt".into(), state: "passed".into(), exit_code: Some(0), output: None }],
        }),
        decisions: vec![],
        usage: Some(Usage { requests: 2, tokens, cost_usd: 0.009, last_ms: NOW - 5_000, transcript: Some(tokens), side_requests: 0, side_tokens: Tokens::default() }),
    };
    let mut waiting = TaskView {
        task: task(
            2,
            TaskState::WaitingPermission,
            "permission #7: Bash: python3 -c 'print(103)'; run `baton decide 7 allow` or `deny`",
            "Run three python commands and report each output",
        ),
        worker: Some(worker(2, "running", "71155bb2", "waiting", 2)),
        candidate: None,
        decisions: vec![Decision {
            id: 7,
            task_id: Some(2),
            attempt_id: Some(2),
            kind: "permission".into(),
            summary: "Bash: python3 -c 'print(103)'".into(),
            options: vec!["allow".into(), "deny".into()],
            status: "pending".into(),
            answer: None,
            note: None,
            created_ms: NOW - 2_000,
            answered_ms: None,
        }],
        usage: None,
    };
    waiting.worker.as_mut().unwrap().waiting_for = Some("permission prompt".into());
    let done = TaskView {
        task: task(1, TaskState::Accepted, "accepted candidate ce23d616e5b1; it's on baton/1 for you to merge", "Create greeting.txt"),
        worker: None,
        candidate: None,
        decisions: vec![],
        usage: None,
    };
    let mut app = App::new(false);
    let quota = Quota { five_hour_pct: Some(85.0), five_hour_resets_at: None, seven_day_pct: Some(22.0), seven_day_resets_at: None, observed_ms: NOW - 3_000 };
    app.set_status(vec![done, review, waiting], Some(quota), NOW);
    app.detail = Some((
        3,
        TaskDetail {
            stat: " usage.txt | 1 +\n 1 file changed, 1 insertion(+)".into(),
            patch: "diff --git a/usage.txt b/usage.txt\nnew file mode 100644\n--- /dev/null\n+++ b/usage.txt\n@@ -0,0 +1 @@\n+ok".into(),
            truncated: false,
            last_message: Some("Done.\n\nSTATUS: done\nSUMMARY: created usage.txt\x1b[2J\x1b[31m".into()),
        },
    ));
    app
}
