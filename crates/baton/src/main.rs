use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use baton::client;
use baton::daemon;
use baton::doctor::{self, Level};
use baton::paths::Paths;
use baton::protocol::{Request, Response};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "baton", version, about = "Coordinates Claude Code workers and delivers reviewable changes")]
struct Cli {
    /// Print machine-readable JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon that owns Baton's state (in the foreground)
    Daemon,
    /// Start a task: one worker in its own worktree, then Baton's own checks
    Task {
        goal: String,
        /// Acceptance check, run by Baton against the exact candidate (repeatable)
        #[arg(long = "check")]
        checks: Vec<String>,
        /// Repository to work in [default: current directory]
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Makes retries safe: a request id never creates a second task [default: random]
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Show tasks: what needs you, what's progressing, what's ready to review
    Status,
    /// Answer a pending decision
    Decide { id: i64, answer: String },
    /// Entry point for Claude Code hooks
    Hook { event: String },
    /// Check Claude Code, git and the repository before dispatching
    Doctor {
        /// Repository to check [default: current directory]
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("baton: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Some(Cmd::Doctor { repo }) => run_doctor(repo, cli.json),
        Some(Cmd::Daemon) => {
            daemon::run(&Paths::from_env()?)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Task { goal, checks, repo, request_id }) => task(goal, checks, repo, request_id, cli.json),
        Some(Cmd::Status) => status(cli.json),
        None => Ok(not_yet("the TUI", "M1.7")),
        Some(Cmd::Decide { .. }) => Ok(not_yet("`baton decide`", "M1.4")),
        Some(Cmd::Hook { .. }) => Ok(not_yet("`baton hook`", "M1.3")),
    }
}

fn not_yet(what: &str, slice: &str) -> ExitCode {
    eprintln!("{what} is not implemented yet (planned for {slice}); see `baton --help`");
    ExitCode::from(2)
}

fn dir_or_cwd(dir: Option<PathBuf>) -> Result<PathBuf> {
    let dir = match dir {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    dir.canonicalize().with_context(|| format!("resolving {}", dir.display()))
}

fn task(goal: String, checks: Vec<String>, repo: Option<PathBuf>, request_id: Option<String>, json: bool) -> Result<ExitCode> {
    let request = Request::CreateTask {
        request_id: request_id.map_or_else(client::new_request_id, Ok)?,
        repo: dir_or_cwd(repo)?,
        goal,
        checks,
    };
    let Response::Task { task, created } = client::call(&Paths::from_env()?, &request)? else {
        bail!("unexpected reply from the daemon");
    };
    if json {
        println!("{}", serde_json::json!({ "task": task, "created": created }));
    } else {
        let note = if created { "" } else { " (already created by this request id)" };
        println!("task {} {}{note}: {}", task.id, task.state.as_str(), task.goal);
    }
    Ok(ExitCode::SUCCESS)
}

fn status(json: bool) -> Result<ExitCode> {
    let Response::Status { tasks } = client::call(&Paths::from_env()?, &Request::Status)? else {
        bail!("unexpected reply from the daemon");
    };
    if json {
        println!("{}", serde_json::json!({ "tasks": tasks }));
    } else if tasks.is_empty() {
        println!("no tasks");
    } else {
        for t in &tasks {
            let repo = t.repo.file_name().unwrap_or_default().to_string_lossy();
            let reason = t.state_reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default();
            println!("{:>4}  {:<18} {repo}: {}{reason}", t.id, t.state.as_str(), t.goal);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run_doctor(repo: Option<PathBuf>, json: bool) -> Result<ExitCode> {
    let dir = match repo {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    let checks = doctor::run(&dir);
    let ok = checks.iter().all(|c| c.level != Level::Fail);
    if json {
        println!("{}", serde_json::json!({ "ok": ok, "checks": checks }));
    } else {
        for c in &checks {
            let level = match c.level {
                Level::Ok => "ok",
                Level::Warn => "warn",
                Level::Fail => "fail",
            };
            println!("{level:<5} {:<7} {}", c.name, c.detail);
            if let Some(fix) = &c.fix {
                println!("{:<13} fix: {fix}", "");
            }
        }
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
