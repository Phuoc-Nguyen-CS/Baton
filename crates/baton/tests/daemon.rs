//! Runs the real `baton` binary: a daemon on a private state directory, driven
//! through the CLI, restarted gracefully and by SIGKILL.

use std::path::{Path, PathBuf};
use std::io::Write as _;
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

    /// Feeds one hook event to `baton hook`, as Claude Code would; returns its stdout.
    fn hook(&self, event: &str, input: Value) -> String {
        let mut child = Command::new(env!("CARGO_BIN_EXE_baton"))
            .env("BATON_HOME", self.home.path())
            .args(["hook", "--attempt", "1", "--role", "baton-worker", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "baton hook must always exit 0");
        String::from_utf8(out.stdout).unwrap()
    }

    /// Polls `status --json` until `done` holds for the first task.
    fn wait_for(&self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = self.json(&["status", "--json"]);
            if done(&status["tasks"][0]) {
                return status["tasks"][0].clone();
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {status}");
            sleep(Duration::from_millis(50));
        }
    }

    /// Never the real backend: tests must not start Claude sessions.
    fn start_daemon(&self) -> Daemon {
        let child = Command::new(env!("CARGO_BIN_EXE_baton"))
            .env("BATON_HOME", self.home.path())
            .args(["daemon", "--backend", "fake"])
            .env("BATON_OTLP_PORT", "0")
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
    let dispatched = env.wait_for("dispatch", |t| t["worker"]["session"].is_string());
    // What a restart must preserve; liveness is re-observed, so it may change.
    let durable = |t: &Value| {
        serde_json::json!([t["id"], t["goal"], t["base_rev"], t["state"], t["worker"]["attempt"], t["worker"]["session"], t["worker"]["worktree"]])
    };
    let before = durable(&dispatched);

    // Graceful stop: the socket goes away and the CLI says so.
    daemon.terminate();
    assert!(!env.paths().socket().exists());
    let out = env.baton(&["status"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("daemon isn't running"));

    let daemon = env.start_daemon();
    assert_eq!(durable(&env.json(&["status", "--json"])["tasks"][0]), before);

    // SIGKILL leaves a stale socket file; the next daemon replaces it.
    daemon.kill();
    assert!(env.paths().socket().exists());
    let _daemon = env.start_daemon();
    assert_eq!(durable(&env.json(&["status", "--json"])["tasks"][0]), before);

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

#[test]
fn telemetry_and_the_status_line_reach_status() {
    let env = Env::new();
    let (repo, _) = env.repo("r");
    let _daemon = env.start_daemon();
    env.json(&["task", "add a greeting", "--repo", repo.to_str().unwrap(), "--json"]);
    let task = env.wait_for("dispatch", |t| t["worker"]["session"].is_string());
    assert_eq!(task["usage"], Value::Null, "unknown before any telemetry");
    let session = format!("{}-0000-4000-8000-000000000000", task["worker"]["session"].as_str().unwrap());

    // The worker's settings point Claude Code's exporter at Baton's receiver.
    let settings: Value = serde_json::from_slice(&std::fs::read(env.home.path().join("attempts/1-1/settings.json")).unwrap()).unwrap();
    let endpoint = settings["env"]["OTEL_EXPORTER_OTLP_ENDPOINT"].as_str().unwrap().to_owned();
    let attr = |k: &str, v: Value| serde_json::json!({ "key": k, "value": v });
    let export = serde_json::json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": [{
        "timeUnixNano": "1791152849189000000",
        "attributes": [
            attr("event.name", serde_json::json!({ "stringValue": "api_request" })),
            attr("session.id", serde_json::json!({ "stringValue": session })),
            attr("user.email", serde_json::json!({ "stringValue": "owner@example.com" })),
            attr("input_tokens", serde_json::json!({ "intValue": 10 })),
            attr("output_tokens", serde_json::json!({ "intValue": 209 })),
            attr("cache_read_tokens", serde_json::json!({ "intValue": 24595 })),
            attr("cache_creation_tokens", serde_json::json!({ "intValue": 9277 })),
            attr("cost_usd", serde_json::json!({ "doubleValue": 0.0220685 })),
            attr("request_id", serde_json::json!({ "stringValue": "req_1" })),
        ],
    }] }] }] });
    let body = export.to_string();
    for _ in 0..2 {
        // Sent twice, as an exporter retry would: it counts once.
        let addr = endpoint.trim_start_matches("http://");
        let mut conn = std::net::TcpStream::connect(addr).unwrap();
        write!(conn, "POST /v1/logs HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut reply = String::new();
        std::io::Read::read_to_string(&mut conn, &mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    }

    let status_line = serde_json::json!({ "session_id": session, "agent_type": "baton-worker",
        "rate_limits": { "five_hour": { "used_percentage": 16, "resets_at": 1791169800 }, "seven_day": { "used_percentage": 13, "resets_at": 1791399600 } } });
    assert_eq!(env.hook("StatusLine", status_line).trim(), "baton · attempt 1 · baton-worker");

    let transcript = env.home.path().join("transcript.jsonl");
    let usage = serde_json::json!({ "input_tokens": 10, "output_tokens": 209, "cache_read_input_tokens": 24595, "cache_creation_input_tokens": 9277 });
    std::fs::write(&transcript, serde_json::json!({ "type": "assistant", "message": { "id": "m1", "usage": usage } }).to_string()).unwrap();
    let stop = serde_json::json!({ "session_id": session, "agent_type": "baton-worker", "transcript_path": transcript, "last_assistant_message": "STATUS: done" });
    env.hook("Stop", stop);

    let t = env.wait_for("usage", |t| t["usage"]["transcript"].is_object());
    assert_eq!(t["usage"]["requests"], 1);
    assert_eq!(t["usage"]["tokens"], serde_json::json!({ "input": 10, "output": 209, "cache_read": 24595, "cache_write": 9277 }));
    assert_eq!(t["usage"]["transcript"], t["usage"]["tokens"]);
    let status = env.json(&["status", "--json"]);
    assert_eq!(status["quota"]["five_hour_pct"], 16.0);
    assert!(!status.to_string().contains("owner@example.com"), "identity never reaches Baton's state");
    let text = String::from_utf8(env.baton(&["status"]).stdout).unwrap();
    assert!(text.contains("quota: 5 h 16% · 7 d 13%"), "{text}");
    assert!(text.contains("usage: 1 requests · 10 in / 209 out / 24595 cache read / 9277 cache write · $0.0221 est. (transcript agrees)"), "{text}");
}

#[test]
fn a_permission_hook_waits_for_baton_decide() {
    let env = Env::new();
    let (repo, _) = env.repo("r");
    let _daemon = env.start_daemon();
    env.json(&["task", "run a script", "--repo", repo.to_str().unwrap(), "--json"]);
    let task = env.wait_for("dispatch", |t| t["worker"]["session"].is_string());
    let input = serde_json::json!({
        "session_id": format!("{}-0000-4000-8000-000000000000", task["worker"]["session"].as_str().unwrap()),
        "hook_event_name": "PermissionRequest",
        "agent_type": "baton-worker",
        "tool_name": "Bash",
        "tool_input": { "command": "python3 -c 'print(1)'", "description": "print" },
    });

    let (hook_out, decide_out) = std::thread::scope(|s| {
        let hook = s.spawn(|| env.hook("PermissionRequest", input));
        let t = env.wait_for("the decision", |t| t["decisions"].as_array().is_some_and(|d| !d.is_empty()));
        assert_eq!(t["state"], "waiting_permission");
        let d = &t["decisions"][0];
        assert_eq!(d["summary"], "Bash: python3 -c 'print(1)'");
        let decide = env.baton(&["decide", &d["id"].to_string(), "allow"]);
        (hook.join().unwrap(), decide)
    });
    assert!(decide_out.status.success(), "{}", String::from_utf8_lossy(&decide_out.stderr));
    assert!(String::from_utf8_lossy(&decide_out.stdout).contains("delivered to the waiting worker"));
    let out: Value = serde_json::from_str(&hook_out).unwrap();
    assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "allow");
    let t = env.wait_for("running again", |t| t["state"] == "running");
    assert_eq!(t["decisions"], serde_json::json!([]));
}

#[test]
fn hooks_reach_the_daemon_and_guards_hold_without_it() {
    let env = Env::new();
    let (repo, _) = env.repo("r");
    let daemon = env.start_daemon();
    env.json(&["task", "add a greeting", "--repo", repo.to_str().unwrap(), "--json"]);
    let task = env.wait_for("dispatch", |t| t["worker"]["session"].is_string());
    let session = format!("{}-0000-4000-8000-000000000000", task["worker"]["session"].as_str().unwrap());
    let event = |name: &str, extra: Value| {
        let mut input = serde_json::json!({ "session_id": session, "hook_event_name": name, "agent_type": "baton-worker" });
        input.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        input
    };
    let push = event("PreToolUse", serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": "git push origin main" } }));

    assert_eq!(env.hook("SessionStart", event("SessionStart", serde_json::json!({ "source": "startup" }))), "");
    let t = env.wait_for("SessionStart", |t| t["worker"]["agent_type"] == "baton-worker");
    assert_eq!(t["state_reason"], "worker session started (startup)");

    let denied: Value = serde_json::from_str(&env.hook("PreToolUse", push.clone())).unwrap();
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
    let t = env.wait_for("the guard's report", |t| t["state_reason"].as_str().unwrap().starts_with("Baton blocked Bash"));
    assert_eq!(t["state"], "running");

    let stop = event("Stop", serde_json::json!({ "last_assistant_message": "STATUS: done" }));
    assert_eq!(env.hook("Stop", stop.clone()), "");
    env.wait_for("Stop", |t| t["worker"]["attempt_state"] == "turn_ended");

    // With the daemon gone, informational events pass silently and guards still deny.
    daemon.terminate();
    assert_eq!(env.hook("Stop", stop), "");
    let denied: Value = serde_json::from_str(&env.hook("PreToolUse", push)).unwrap();
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
}
