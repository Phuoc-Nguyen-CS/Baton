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
    pub state: TaskState,
    pub state_reason: Option<String>,
    pub observed_ms: i64,
    pub created_ms: i64,
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
