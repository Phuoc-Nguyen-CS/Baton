//! The few git queries Baton needs, run through the `git` CLI.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
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

/// Makes sure git ignores `pattern` in `repo`, probing with the path `probe`. Adds
/// it to the repo's local exclude file, so the owner's tracked files stay untouched.
pub fn ensure_ignored(repo: &Path, pattern: &str, probe: &str) -> Result<()> {
    let status = Command::new("git").arg("-C").arg(repo).args(["check-ignore", "-q", probe]).status()?;
    match status.code() {
        Some(0) => return Ok(()),
        Some(1) => {}
        _ => bail!("`git check-ignore` failed in {} ({status})", repo.display()),
    }
    let common = output(Command::new("git").arg("-C").arg(repo).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]))?;
    let exclude = PathBuf::from(common).join("info").join("exclude");
    fs::create_dir_all(exclude.parent().expect("has a parent"))?;
    let existing = fs::read_to_string(&exclude).unwrap_or_default();
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    let mut file = OpenOptions::new().create(true).append(true).open(&exclude)?;
    writeln!(file, "{separator}{pattern}")?;
    Ok(())
}

/// Creates a linked worktree at `path` on a new `branch` starting at `base`.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base: &str) -> Result<()> {
    output(Command::new("git").arg("-C").arg(repo).args(["worktree", "add", "-q", "-b", branch]).arg(path).arg(base))?;
    Ok(())
}

/// Commits everything in `worktree` (tracked, new and deleted files; ignored ones
/// stay out) as Baton, skipping the repo's commit hooks. Returns the resulting
/// HEAD commit and tree; with nothing to commit, HEAD's.
pub fn snapshot(worktree: &Path, message: &str) -> Result<(String, String)> {
    let git = || {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(worktree);
        cmd
    };
    output(git().args(["add", "-A"]))?;
    let staged = git().args(["diff", "--cached", "--quiet"]).status()?;
    if staged.code() == Some(1) {
        output(
            git()
                .args(["-c", "user.name=Baton", "-c", "user.email=baton@localhost", "-c", "commit.gpgsign=false"])
                .args(["commit", "-q", "--no-verify", "-m", message]),
        )?;
    } else if !staged.success() {
        bail!("`git diff --cached` failed in {} ({staged})", worktree.display());
    }
    let commit = output(git().args(["rev-parse", "HEAD"]))?;
    let tree = output(git().args(["rev-parse", "HEAD^{tree}"]))?;
    Ok((commit, tree))
}

/// Whether `worktree` still holds exactly `commit`: checked out, nothing changed or added.
pub fn is_at(worktree: &Path, commit: &str) -> Result<bool> {
    let head = output(Command::new("git").arg("-C").arg(worktree).args(["rev-parse", "HEAD"]))?;
    let status = output(Command::new("git").arg("-C").arg(worktree).args(["status", "--porcelain"]))?;
    Ok(head == commit && status.is_empty())
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

    #[test]
    fn ignoring_uses_the_local_exclude_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("r");
        std::fs::create_dir(&root).unwrap();
        git_in(&root, &["init", "-q"]);
        let exclude = root.join(".git/info/exclude");
        std::fs::write(&exclude, "# no trailing newline").unwrap();

        ensure_ignored(&root, ".claude/worktrees/", ".claude/worktrees/probe").unwrap();
        ensure_ignored(&root, ".claude/worktrees/", ".claude/worktrees/probe").unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "# no trailing newline\n.claude/worktrees/\n");
        assert!(!root.join(".gitignore").exists(), "the owner's files stay untouched");

        git_in(&root, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let head = repo(&root).unwrap().head;
        let wt = root.join(".claude/worktrees/baton-1-1");
        add_worktree(&root, &wt, "baton/1", &head).unwrap();
        assert_eq!(repo(&wt).unwrap().head, head);
        let status = output(Command::new("git").arg("-C").arg(&root).args(["status", "--porcelain"])).unwrap();
        assert_eq!(status, "", "the worktree doesn't show up in the main checkout");
        assert!(add_worktree(&root, &root.join(".claude/worktrees/again"), "baton/1", &head).is_err(), "branch exists");
    }

    #[test]
    fn snapshot_commits_everything_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("r");
        std::fs::create_dir(&root).unwrap();
        git_in(&root, &["init", "-q"]);
        std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
        std::fs::write(root.join("gone.txt"), "gone\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "init"]);
        let base = repo(&root).unwrap().head;

        // Nothing changed: the candidate is HEAD.
        let (commit, _) = snapshot(&root, "baton: candidate").unwrap();
        assert_eq!(commit, base);
        assert!(is_at(&root, &commit).unwrap());

        std::fs::write(root.join("keep.txt"), "changed\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::create_dir(root.join("build")).unwrap();
        std::fs::write(root.join("build/out"), "ignored\n").unwrap();
        assert!(!is_at(&root, &base).unwrap());

        let (commit, tree) = snapshot(&root, "baton: candidate").unwrap();
        assert_ne!(commit, base);
        assert!(is_at(&root, &commit).unwrap());
        let files = output(Command::new("git").arg("-C").arg(&root).args(["ls-tree", "--name-only", &tree])).unwrap();
        assert_eq!(files, ".gitignore\nkeep.txt\nnew.txt");
        let author = output(Command::new("git").arg("-C").arg(&root).args(["log", "-1", "--format=%an <%ae>"])).unwrap();
        assert_eq!(author, "Baton <baton@localhost>");

        std::fs::write(root.join("new.txt"), "edited after the snapshot\n").unwrap();
        assert!(!is_at(&root, &commit).unwrap(), "a later edit means the worktree no longer holds the candidate");
    }
}
