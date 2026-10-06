//! Types shared by the store, the daemon protocol and the CLI.

use std::path::PathBuf;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

/// Baton's own view of a task, derived from hooks and polling, never from
/// Claude's `state` field (architecture §4, F19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    WaitingPermission,
    WaitingInput,
    Verifying,
    ReviewReady,
    Accepted,
    Failed,
    Cancelled,
    Reconciling,
}

impl TaskState {
    const ALL: [TaskState; 10] = [
        TaskState::Queued,
        TaskState::Running,
        TaskState::WaitingPermission,
        TaskState::WaitingInput,
        TaskState::Verifying,
        TaskState::ReviewReady,
        TaskState::Accepted,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Reconciling,
    ];

    /// Final states: nothing observed later changes them.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Accepted | TaskState::Failed | TaskState::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Running => "running",
            TaskState::WaitingPermission => "waiting_permission",
            TaskState::WaitingInput => "waiting_input",
            TaskState::Verifying => "verifying",
            TaskState::ReviewReady => "review_ready",
            TaskState::Accepted => "accepted",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
            TaskState::Reconciling => "reconciling",
        }
    }
}

impl ToSql for TaskState {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for TaskState {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        TaskState::ALL
            .into_iter()
            .find(|state| state.as_str() == s)
            .ok_or_else(|| FromSqlError::Other(format!("unknown task state {s:?}").into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub request_id: String,
    pub repo: PathBuf,
    pub base_rev: String,
    pub goal: String,
    pub checks: Vec<String>,
    pub model: Option<String>,
    pub state: TaskState,
    pub state_reason: Option<String>,
    pub observed_ms: i64,
    pub created_ms: i64,
}

/// A task with its latest attempt, candidate and open decisions, as `status` shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    pub worker: Option<Worker>,
    pub candidate: Option<Candidate>,
    /// Pending decisions: what needs the owner.
    pub decisions: Vec<Decision>,
    /// Across all attempts; `None` means unknown (no telemetry yet), never zero.
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
}

impl std::ops::AddAssign for Tokens {
    fn add_assign(&mut self, o: Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
}

/// Usage from OpenTelemetry `api_request` events (D6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub requests: i64,
    pub tokens: Tokens,
    /// Claude Code's client-side estimate, not a bill.
    pub cost_usd: f64,
    /// When the newest request was made.
    pub last_ms: i64,
    /// The same sessions' transcripts, read at their last `Stop`; `None` until
    /// every session with usage has been read.
    pub transcript: Option<Tokens>,
    /// Requests in `tokens` that no transcript lists, such as Claude Code's
    /// `away_summary` recap of an idle session (M1.9): billed, but outside the
    /// cross-check.
    pub side_requests: i64,
    pub side_tokens: Tokens,
}

impl Usage {
    /// The transcript cross-check, for display.
    pub fn cross_check(&self) -> String {
        let Some(mut expected) = self.transcript else { return "transcript not read yet".into() };
        expected += self.side_tokens;
        let verdict = if expected == self.tokens { "transcript agrees" } else { "transcript differs" };
        match self.side_requests {
            0 => verdict.into(),
            1 => format!("{verdict}; 1 side request"),
            n => format!("{verdict}; {n} side requests"),
        }
    }
}

/// A transcript's token totals and the request ids they came from. Rows stored
/// before M1.9 have no ids.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptUsage {
    #[serde(flatten)]
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<String>,
}

/// The review screen's evidence for a task's newest candidate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskDetail {
    /// `git diff --stat` from the base to the candidate.
    pub stat: String,
    pub patch: String,
    /// The patch was cut to fit; the full diff is on `baton/<task>`.
    pub truncated: bool,
    /// The worker's last reply, with its handoff. Untrusted text.
    pub last_message: Option<String>,
}

/// The account's quota from a worker's status line, with the time it was seen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    pub five_hour_pct: Option<f64>,
    /// Unix seconds.
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<i64>,
    pub observed_ms: i64,
}

/// A durable request for the owner (PLAN §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub id: i64,
    pub task_id: Option<i64>,
    pub attempt_id: Option<i64>,
    /// permission | review
    pub kind: String,
    pub summary: String,
    pub options: Vec<String>,
    /// pending | answered | applied | expired | withdrawn
    pub status: String,
    pub answer: Option<String>,
    pub note: Option<String>,
    pub created_ms: i64,
    pub answered_ms: Option<i64>,
}

/// The exact revision Baton verified, with the latest result of each check on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: i64,
    pub commit: String,
    pub tree: String,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub command: String,
    /// running | passed | failed | error | invalidated
    pub state: String,
    pub exit_code: Option<i32>,
    /// Captured output.
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worker {
    pub attempt: i64,
    pub attempt_state: String,
    pub worktree: PathBuf,
    pub branch: String,
    /// The dispatched session's short id, once known.
    pub session: Option<String>,
    /// `claude`, or `fake` in tests and demos.
    pub backend: Option<String>,
    pub liveness: Option<String>,
    pub waiting_for: Option<String>,
    pub agent_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_names_match_json_names() {
        for state in TaskState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
    }
}
