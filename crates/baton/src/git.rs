//! The few git queries Baton needs, run through the `git` CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

pub struct Repo {
    /// The main worktree. Claude trusts linked worktrees through it (compat-record F8).
    pub main: PathBuf,
    /// The commit checked out in the directory that was asked about.
    pub head: String,
}

pub fn repo(dir: &Path) -> Result<Repo> {
    let list = output(Command::new("git").arg("-C").arg(dir).args(["worktree", "list", "--porcelain"]))?;
    let main = main_worktree(&list)?;
    let head = output(Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--verify", "-q", "HEAD^{commit}"]))
        .map_err(|_| anyhow!("no commits yet"))?;
    Ok(Repo { main, head })
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

/// Runs a command and returns its trimmed stdout, or an error carrying its stderr.
pub fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!("{} ({})", String::from_utf8_lossy(&out.stderr).trim(), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
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
        assert_eq!(repo(&root).err().unwrap().to_string(), "no commits yet");

        git_in(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let r = repo(&root).unwrap();
        assert_eq!(r.main, root);
        assert_eq!(r.head.len(), 40);

        let linked = root.join(".claude/worktrees/w");
        git_in(&root, &["worktree", "add", "-q", "--detach", linked.to_str().unwrap()]);
        git_in(&linked, &["commit", "-q", "--allow-empty", "-m", "on the linked worktree"]);
        let l = repo(&linked).unwrap();
        assert_eq!(l.main, root);
        assert_ne!(l.head, r.head, "head is the asked-about directory's");

        assert!(repo(dir.path()).is_err(), "a plain directory isn't a repo");
    }
}
