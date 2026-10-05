//! Messages on the daemon's Unix socket: one JSON request line, one JSON response
//! line, per connection.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Decision, Quota, Task, TaskDetail, TaskView};

/// Upper bound for one message line, either direction.
pub const MAX_MESSAGE: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    CreateTask {
        request_id: String,
        /// Absolute path inside the repository.
        repo: PathBuf,
        goal: String,
        checks: Vec<String>,
        model: Option<String>,
    },
    Status,
    /// A worker's hook event, already summarized by `baton hook`.
    Hook {
        attempt: i64,
        event: String,
        input: Value,
        /// Set when `baton hook`'s own guard denied the tool call.
        denied: Option<String>,
    },
    /// The owner's answer to a pending decision.
    Decide { id: i64, answer: String, note: Option<String> },
    /// What the review screen shows beyond `status`: the diff and the handoff.
    Detail { task: i64 },
    /// The owner's verdict on a task's candidate. `candidate` is the commit the
    /// owner reviewed; a verdict on any other commit is refused (PLAN §6).
    Review { task: i64, verdict: Verdict, candidate: Option<String>, note: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Accept the candidate; it stays on `baton/<task>` for the owner to merge
    Accept,
    /// Ask the worker for changes (with --note)
    Changes,
    /// Decide later
    Defer,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Pong { version: String, pid: u32 },
    Task { task: Task, created: bool },
    Status { tasks: Vec<TaskView>, quota: Option<Quota> },
    /// JSON for `baton hook` to print for Claude Code, if any.
    Hook { output: Option<Value> },
    /// How the answer reaches the worker.
    Decided { decision: Decision, delivery: String },
    Detail { detail: TaskDetail },
    Reviewed { task: Task, outcome: String },
    Error { message: String },
}
