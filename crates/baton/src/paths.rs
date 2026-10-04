use std::path::PathBuf;

use anyhow::{Result, anyhow};

/// Where Baton keeps its state: `$BATON_HOME`, else `$XDG_STATE_HOME/baton`,
/// else `~/.local/state/baton`.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let var = |k| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = var("BATON_HOME")
            .or_else(|| var("XDG_STATE_HOME").map(|d| d.join("baton")))
            .or_else(|| var("HOME").map(|h| h.join(".local/state/baton")))
            .ok_or_else(|| anyhow!("can't place Baton's state: set BATON_HOME or HOME"))?;
        Ok(Self { home })
    }

    pub fn db(&self) -> PathBuf {
        self.home.join("baton.db")
    }

    pub fn socket(&self) -> PathBuf {
        self.home.join("daemon.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.home.join("daemon.lock")
    }

    /// Baton's own files for one attempt: the worker's settings, its last message.
    pub fn attempt_dir(&self, task_id: i64, seq: i64) -> PathBuf {
        self.home.join("attempts").join(format!("{task_id}-{seq}"))
    }
}
