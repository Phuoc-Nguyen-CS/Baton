use std::path::PathBuf;
use std::process::ExitCode;

use baton::doctor::{self, Level};
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
    /// Run the daemon that owns Baton's state
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
    match cli.command {
        Some(Cmd::Doctor { repo }) => run_doctor(repo, cli.json),
        None => not_yet("the TUI", "M1.7"),
        Some(Cmd::Daemon) => not_yet("`baton daemon`", "M1.2"),
        Some(Cmd::Task { .. }) => not_yet("`baton task`", "M1.2"),
        Some(Cmd::Status) => not_yet("`baton status`", "M1.2"),
        Some(Cmd::Decide { .. }) => not_yet("`baton decide`", "M1.4"),
        Some(Cmd::Hook { .. }) => not_yet("`baton hook`", "M1.3"),
    }
}

fn not_yet(what: &str, slice: &str) -> ExitCode {
    eprintln!("{what} is not implemented yet (planned for {slice}); see `baton --help`");
    ExitCode::from(2)
}

fn run_doctor(repo: Option<PathBuf>, json: bool) -> ExitCode {
    let dir = match repo.map_or_else(std::env::current_dir, Ok) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("baton doctor: {e}");
            return ExitCode::FAILURE;
        }
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
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
