//! `baton doctor`: checks the local setup Baton depends on, without starting a session.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use serde::Serialize;

use crate::git::{self, output};

/// The Claude Code version docs/compat-record.md and the spikes were run against.
pub const TESTED_CLAUDE_VERSION: &str = "2.1.289";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    fn new(name: &'static str, level: Level, detail: impl Into<String>) -> Self {
        Self { name, level, detail: detail.into(), fix: None }
    }

    fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

pub fn run(dir: &Path) -> Vec<Check> {
    let mut checks = vec![claude(), git()];
    match git::repo(dir) {
        Ok(repo) => {
            checks.push(Check::new("repo", Level::Ok, format!("{} at {}", repo.main.display(), &repo.head[..12])));
            let config = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude.json"));
            checks.push(match config {
                Some(config) => trust(&repo.main, &config),
                None => Check::new("trust", Level::Warn, "unknown: HOME is not set"),
            });
        }
        Err(e) => checks.push(
            Check::new("repo", Level::Fail, format!("{}: {e:#}", dir.display()))
                .fix("run Baton inside a git repository that has at least one commit"),
        ),
    }
    checks
}

fn claude() -> Check {
    match output(Command::new("claude").arg("--version")) {
        Ok(out) => {
            let version = out.split_whitespace().next().unwrap_or_default();
            if version == TESTED_CLAUDE_VERSION {
                Check::new("claude", Level::Ok, format!("{version} (the version Baton was tested with)"))
            } else {
                Check::new("claude", Level::Warn, format!("{version}, untested (Baton was tested with {TESTED_CLAUDE_VERSION})"))
                    .fix("re-run the spikes in spikes/ and record the results in docs/compat-record.md")
            }
        }
        Err(e) => Check::new("claude", Level::Fail, format!("can't run `claude --version`: {e:#}"))
            .fix("install Claude Code and log in with `claude`"),
    }
}

fn git() -> Check {
    match output(Command::new("git").arg("--version")) {
        Ok(out) => Check::new("git", Level::Ok, out.trim_start_matches("git version ")),
        Err(e) => Check::new("git", Level::Fail, format!("can't run `git --version`: {e:#}")),
    }
}

/// Best effort: reads Claude Code's undocumented config file without changing it,
/// keeping only the repo's trust flag. Dispatch reports the authoritative answer (C15).
fn trust(repo: &Path, config: &Path) -> Check {
    let fix = format!("run `claude` in {} once and accept the trust prompt", repo.display());
    let source = format!("per {}, best effort", config.display());
    let parsed = std::fs::read_to_string(config)
        .with_context(|| format!("reading {}", config.display()))
        .and_then(|text| Ok(serde_json::from_str::<serde_json::Value>(&text)?));
    let value = match parsed {
        Ok(value) => value,
        Err(e) => return Check::new("trust", Level::Warn, format!("unknown: {e:#}")),
    };
    let key = repo.to_string_lossy();
    match value["projects"][key.as_ref()]["hasTrustDialogAccepted"].as_bool() {
        Some(true) => Check::new("trust", Level::Ok, format!("Claude Code trusts this repo ({source})")),
        _ => Check::new("trust", Level::Fail, format!("Claude Code doesn't trust this repo yet ({source})")).fix(fix),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_reads_only_the_repo_flag() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        let level = |text: Option<&str>, repo: &str| {
            match text {
                Some(t) => std::fs::write(&config, t).unwrap(),
                None => drop(std::fs::remove_file(&config)),
            }
            trust(Path::new(repo), &config).level
        };
        let json = r#"{"projects": {"/a": {"hasTrustDialogAccepted": true}, "/b": {"hasTrustDialogAccepted": false}}}"#;
        assert_eq!(level(Some(json), "/a"), Level::Ok);
        assert_eq!(level(Some(json), "/b"), Level::Fail);
        assert_eq!(level(Some(json), "/c"), Level::Fail);
        assert_eq!(level(Some("not json"), "/a"), Level::Warn);
        assert_eq!(level(None, "/a"), Level::Warn);
    }
}
