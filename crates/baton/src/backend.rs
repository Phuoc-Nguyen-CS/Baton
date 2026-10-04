//! The seam between Baton and an agent CLI. Only Claude Code is implemented
//! (docs/decisions.md); `fake` is the deterministic stand-in for tests (PLAN.md §10).
//!
//! Calls are blocking: each one is a short CLI invocation, which the daemon runs off
//! its event loop.

pub mod fake;

use std::path::PathBuf;

use anyhow::Result;

/// Everything a backend needs to start one worker for one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRequest {
    /// Session name, `baton-<task>-<attempt>`.
    pub name: String,
    /// The Baton-made worktree the worker runs in (D1).
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    /// Role definition passed inline (D3).
    pub role: Option<Role>,
    /// Baton-written settings file: hooks, deny rules, telemetry (architecture §2).
    pub settings: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub name: String,
    /// Backend-specific definition, e.g. Claude's `--agents` JSON.
    pub definition: String,
}

/// A backend's identity for one session. `uuid` may only be known after listing (F11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub short_id: String,
    pub uuid: Option<String>,
}

/// Whether the session's process is alive, from polling (C2). Never a "finished" signal (F19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    Busy,
    Waiting,
    Idle,
    NotRunning,
    Unknown(String),
}

impl Liveness {
    pub fn is_running(&self) -> bool {
        matches!(self, Liveness::Busy | Liveness::Waiting | Liveness::Idle)
    }
}

/// One row of a backend's session listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub session: SessionRef,
    pub name: Option<String>,
    pub pid: Option<u32>,
    pub liveness: Liveness,
    pub waiting_for: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// What a resume actually did. A copy has the original's conversation under a new
/// id and must be stopped (F9, F10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumed {
    Same(SessionRef),
    Copy(SessionRef),
}

pub trait Backend: Send + Sync {
    /// Installed backend version, e.g. `2.1.289`.
    fn version(&self) -> Result<String>;
    fn dispatch(&self, req: &DispatchRequest) -> Result<SessionRef>;
    fn list(&self) -> Result<Vec<Observation>>;
    fn stop(&self, session: &SessionRef) -> Result<()>;
    /// Wakes a stopped session with `prompt`, keeping its saved options.
    fn resume(&self, session: &SessionRef, prompt: &str) -> Result<Resumed>;
}
