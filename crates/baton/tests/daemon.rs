//! Runs the real `baton` binary: a daemon on a private state directory, driven
//! through the CLI, restarted gracefully and by SIGKILL.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use baton::client;
use baton::paths::Paths;
use baton::protocol::Request;
use serde_json::Value;

struct Env {
    home: tempfile::TempDir,
    repos: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self { home: tempfile::tempdir().unwrap(), repos: tempfile::tempdir().unwrap() }
    }

    fn paths(&self) -> Paths {
        Paths { home: self.home.path().to_owned() }
    }

    fn baton(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_baton"))
            .env("BATON_HOME", self.home.path())
            .args(args)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.baton(args);
        assert!(out.status.success(), "baton {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// A repository with one commit; returns its path and HEAD.
    fn repo(&self, name: &str) -> (PathBuf, String) {
        let path = self.repos.path().canonicalize().unwrap().join(name);
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q"]);
        git(&path, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let head = git(&path, &["rev-parse", "HEAD"]);
        (path, head)
    }

    fn start_daemon(&self) -> Daemon {
        let child = Command::new(env!("CARGO_BIN_EXE_baton"))
            .env("BATON_HOME", self.home.path())
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while client::call(&self.paths(), &Request::Ping).is_err() {
            assert!(Instant::now() < deadline, "daemon didn't start");
            sleep(Duration::from_millis(20));
        }
        Daemon(child)
    }
}

struct Daemon(Child);

impl Daemon {
    fn terminate(mut self) {
        let pid = self.0.id().to_string();
        assert!(Command::new("kill").args(["-TERM", &pid]).status().unwrap().success());
        assert!(self.0.wait().unwrap().success(), "daemon exited with an error on SIGTERM");
    }

    fn kill(mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn tasks_survive_daemon_restarts() {
    let env = Env::new();
    let (repo, head) = env.repo("r");
    let repo_arg = repo.to_str().unwrap();

    let daemon = env.start_daemon();
    let created = env.json(&["task", "add a greeting", "--check", "test -f greeting.txt", "--repo", repo_arg, "--json"]);
    assert_eq!(created["created"], true);
    let task = &created["task"];
    assert_eq!(task["state"], "queued");
    assert_eq!(task["repo"], repo_arg);
    assert_eq!(task["base_rev"], head.as_str());
    assert_eq!(task["checks"], serde_json::json!(["test -f greeting.txt"]));
    let before = env.json(&["status", "--json"]);
    assert_eq!(before["tasks"].as_array().unwrap().len(), 1);

    // Graceful stop: the socket goes away and the CLI says so.
    daemon.terminate();
    assert!(!env.paths().socket().exists());
    let out = env.baton(&["status"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("daemon isn't running"));

    let daemon = env.start_daemon();
    assert_eq!(env.json(&["status", "--json"]), before);

    // SIGKILL leaves a stale socket file; the next daemon replaces it.
    daemon.kill();
    assert!(env.paths().socket().exists());
    let _daemon = env.start_daemon();
    assert_eq!(env.json(&["status", "--json"]), before);

    // Intake from a linked worktree records the main worktree as the repo.
    let linked = repo.join(".claude/worktrees/w");
    git(&repo, &["worktree", "add", "-q", "--detach", linked.to_str().unwrap()]);
    let second = env.json(&["task", "second", "--repo", linked.to_str().unwrap(), "--json"]);
    assert_eq!(second["task"]["repo"], repo_arg);
    assert_eq!(second["task"]["id"], 2);
}

#[test]
fn a_request_id_creates_at_most_one_task() {
    let env = Env::new();
    let (repo, _) = env.repo("r");
    let _daemon = env.start_daemon();
    let args = ["task", "add a greeting", "--repo", repo.to_str().unwrap(), "--request-id", "req-1"];

    let first = env.baton(&args);
    assert!(first.status.success());
    let again = env.baton(&args);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("already created by this request id"));

    let mut other = args;
    other[1] = "a different goal";
    let clash = env.baton(&other);
    assert!(!clash.status.success());
    assert!(String::from_utf8_lossy(&clash.stderr).contains("already used for task 1"));

    assert_eq!(env.json(&["status", "--json"])["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn only_one_daemon_owns_a_state_directory() {
    let env = Env::new();
    let _daemon = env.start_daemon();
    let second = env.baton(&["daemon"]);
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));
    // The first daemon still serves.
    assert!(client::call(&env.paths(), &Request::Ping).is_ok());
}

#[test]
fn bad_intake_creates_nothing() {
    let env = Env::new();
    let (repo, _) = env.repo("r");
    let _daemon = env.start_daemon();

    let not_a_repo = env.baton(&["task", "goal", "--repo", env.repos.path().to_str().unwrap()]);
    assert!(!not_a_repo.status.success());
    assert!(String::from_utf8_lossy(&not_a_repo.stderr).contains("isn't a usable git repository"));

    let empty = env.baton(&["task", "  ", "--repo", repo.to_str().unwrap()]);
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("the goal is empty"));

    assert_eq!(env.json(&["status", "--json"])["tasks"], serde_json::json!([]));
}
