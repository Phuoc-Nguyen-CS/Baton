//! `baton doctor`: checks the local setup Baton depends on, without starting a session.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

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
    match repo(dir) {
        Ok((path, head)) => {
            checks.push(Check::new("repo", Level::Ok, format!("{} at {}", path.display(), &head[..12])));
            let config = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude.json"));
            checks.push(match config {
                Some(config) => trust(&path, &config),
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

/// The repository's main worktree and its HEAD commit. Claude trusts linked
/// worktrees through their main worktree (compat-record F8).
fn repo(dir: &Path) -> Result<(PathBuf, String)> {
    let list = output(Command::new("git").arg("-C").arg(dir).args(["worktree", "list", "--porcelain"]))?;
    let path = main_worktree(&list)?;
    let head = output(Command::new("git").arg("-C").arg(&path).args(["rev-parse", "--verify", "-q", "HEAD^{commit}"]))
        .map_err(|_| anyhow!("no commits yet"))?;
    Ok((path, head))
}

/// Parses the first entry of `git worktree list --porcelain`.
fn main_worktree(porcelain: &str) -> Result<PathBuf> {
    let mut entry = porcelain.lines().take_while(|l| !l.is_empty());
    let path = entry
        .next()
        .and_then(|l| l.strip_prefix("worktree "))
        .ok_or_else(|| anyhow!("unexpected `git worktree list` output"))?;
    if entry.any(|l| l == "bare") {
        bail!("bare repository; Baton needs a working tree");
    }
    Ok(PathBuf::from(path))
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

/// Runs a command and returns its trimmed stdout, or an error carrying its stderr.
fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!("{} ({})", String::from_utf8_lossy(&out.stderr).trim(), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    #[test]
    fn main_worktree_is_the_first_entry() {
        let porcelain = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r/.claude/worktrees/x\nHEAD abc\ndetached\n";
        assert_eq!(main_worktree(porcelain).unwrap(), PathBuf::from("/r"));
        assert!(main_worktree("worktree /r.git\nbare\n").is_err());
        assert!(main_worktree("").is_err());
    }

    #[test]
    fn repo_needs_a_commit_and_resolves_linked_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("r");
        std::fs::create_dir(&root).unwrap();
        git_in(&root, &["init", "-q"]);
        assert_eq!(repo(&root).unwrap_err().to_string(), "no commits yet");

        git_in(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let (path, head) = repo(&root).unwrap();
        assert_eq!(path, root);
        assert_eq!(head.len(), 40);

        let linked = root.join(".claude/worktrees/w");
        git_in(&root, &["worktree", "add", "-q", "--detach", linked.to_str().unwrap()]);
        assert_eq!(repo(&linked).unwrap().0, root);

        assert!(repo(dir.path()).is_err(), "a plain directory isn't a repo");
    }

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
