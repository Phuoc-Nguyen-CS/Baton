//! Messages on the daemon's Unix socket: one JSON request line, one JSON response
//! line, per connection.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Task, TaskView};

/// Upper bound for one message line, either direction.
pub const MAX_MESSAGE: usize = 1 << 20;

#[derive(Debug, Serialize, Deserialize)]
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
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Pong { version: String, pid: u32 },
    Task { task: Task, created: bool },
    Status { tasks: Vec<TaskView> },
    /// JSON for `baton hook` to print for Claude Code, if any.
    Hook { output: Option<Value> },
    Error { message: String },
}
