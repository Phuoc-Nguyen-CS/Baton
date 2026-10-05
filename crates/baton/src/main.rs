use std::path::PathBuf;
use std::process::ExitCode;

use std::io::Read as _;

use anyhow::{Context, Result, bail};
use baton::client;
use baton::daemon::{self, BackendKind};
use baton::hook;
use baton::doctor::{self, Level};
use baton::paths::Paths;
use baton::protocol::{Request, Response, Verdict};
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
    Daemon {
        /// Which agent backend runs workers
        #[arg(long, value_enum, default_value = "claude")]
        backend: BackendKind,
    },
    /// Start a task: one worker in its own worktree, then Baton's own checks
    Task {
        goal: String,
        /// Acceptance check, run by Baton against the exact candidate (repeatable)
        #[arg(long = "check")]
        checks: Vec<String>,
        /// Repository to work in [default: current directory]
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Worker model, e.g. haiku [default: Claude Code's]
        #[arg(long)]
        model: Option<String>,
        /// Makes retries safe: a request id never creates a second task [default: random]
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Show tasks: what needs you, what's progressing, what's ready to review
    Status,
    /// Answer a pending decision
    Decide {
        id: i64,
        /// One of the decision's options, e.g. allow or deny
        answer: String,
        /// Message for the worker, e.g. why a request was denied
        #[arg(long)]
        note: Option<String>,
    },
    /// Give your verdict on a task's candidate
    Review {
        task: i64,
        #[arg(value_enum)]
        verdict: Verdict,
        /// The candidate commit you reviewed; refused if it has changed [default: the current one]
        #[arg(long)]
        candidate: Option<String>,
        /// For `changes`: what to change. Otherwise a note for the record
        #[arg(long)]
        note: Option<String>,
    },
    /// Entry point for a worker's Claude Code hooks (written into its settings by Baton)
    #[command(hide = true)]
    Hook {
        /// Claude Code hook event, e.g. PreToolUse
        event: String,
        #[arg(long)]
        attempt: i64,
        /// The role the worker must run as
        #[arg(long)]
        role: String,
    },
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
        Some(Cmd::Daemon { backend }) => {
            daemon::run(&Paths::from_env()?, backend)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Task { goal, checks, repo, model, request_id }) => {
            let request = Request::CreateTask {
                request_id: request_id.map_or_else(client::new_request_id, Ok)?,
                repo: dir_or_cwd(repo)?,
                goal,
                checks,
                model,
            };
            task(&request, cli.json)
        }
        Some(Cmd::Status) => status(cli.json),
        Some(Cmd::Hook { event, attempt, role }) => Ok(run_hook(&event, attempt, &role)),
        None => {
            baton::tui::run(&Paths::from_env()?)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Decide { id, answer, note }) => decide(id, answer, note, cli.json),
        Some(Cmd::Review { task, verdict, candidate, note }) => {
            let request = Request::Review { task, verdict, candidate, note };
            let Response::Reviewed { task, outcome } = client::call(&Paths::from_env()?, &request)? else {
                bail!("unexpected reply from the daemon");
            };
            if cli.json {
                println!("{}", serde_json::json!({ "task": task, "outcome": outcome }));
            } else {
                println!("task {}: {outcome}", task.id);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Always exits 0: exit code 2 would block the worker, and denials go through
/// the JSON output instead.
fn run_hook(event: &str, attempt: i64, role: &str) -> ExitCode {
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("baton hook: reading stdin: {e}");
    }
    let paths = Paths::from_env().inspect_err(|e| eprintln!("baton hook: {e:#}")).ok();
    if let Some(output) = hook::run(paths.as_ref(), attempt, role, event, &stdin) {
        println!("{output}");
    }
    if event == "StatusLine" {
        // What an attached owner sees at the bottom of the worker's screen.
        println!("baton · attempt {attempt} · {role}");
    }
    ExitCode::SUCCESS
}

fn dir_or_cwd(dir: Option<PathBuf>) -> Result<PathBuf> {
    let dir = match dir {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    dir.canonicalize().with_context(|| format!("resolving {}", dir.display()))
}

fn task(request: &Request, json: bool) -> Result<ExitCode> {
    let Response::Task { task, created } = client::call(&Paths::from_env()?, request)? else {
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
    let Response::Status { tasks, quota } = client::call(&Paths::from_env()?, &Request::Status)? else {
        bail!("unexpected reply from the daemon");
    };
    if json {
        println!("{}", serde_json::json!({ "tasks": tasks, "quota": quota }));
        return Ok(ExitCode::SUCCESS);
    }
    match &quota {
        Some(q) => {
            let pct = |p: Option<f64>| p.map_or("?".into(), |p| format!("{p:.0}%"));
            let age = (baton::store::now_ms() - q.observed_ms) / 1000;
            println!("quota: 5 h {} · 7 d {} (seen {age} s ago on a worker's status line)", pct(q.five_hour_pct), pct(q.seven_day_pct));
        }
        None => println!("quota: unknown (no worker status line seen yet)"),
    }
    if tasks.is_empty() {
        println!("no tasks");
    } else {
        for view in &tasks {
            let t = &view.task;
            let repo = t.repo.file_name().unwrap_or_default().to_string_lossy();
            let reason = t.state_reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default();
            println!("{:>4}  {:<18} {repo}: {}{reason}", t.id, t.state.as_str(), t.goal);
            if let Some(w) = &view.worker {
                let session = match (&w.session, &w.liveness) {
                    (Some(id), Some(l)) => format!("session {id} {l}"),
                    (Some(id), None) => format!("session {id}"),
                    _ => "no session yet".into(),
                };
                println!("      attempt {} {} · {session} · {}", w.attempt, w.attempt_state, w.worktree.display());
                match &view.usage {
                    Some(u) => {
                        let t = u.tokens;
                        let check = match u.transcript {
                            Some(tr) if tr == t => "transcript agrees",
                            Some(_) => "transcript differs",
                            None => "transcript not read yet",
                        };
                        println!(
                            "      usage: {} requests · {} in / {} out / {} cache read / {} cache write · ${:.4} est. ({check})",
                            u.requests, t.input, t.output, t.cache_read, t.cache_write, u.cost_usd
                        );
                    }
                    None => println!("      usage: unknown (no telemetry yet)"),
                }
            }
            for d in &view.decisions {
                println!("      needs you: #{} {}: {} → baton decide {} {}", d.id, d.kind, d.summary, d.id, d.options.join("|"));
            }
            if let Some(c) = &view.candidate {
                println!("      candidate {} on baton/{}", &c.commit[..12], t.id);
                for check in &c.checks {
                    let code = check.exit_code.map(|c| format!(" (exit {c})")).unwrap_or_default();
                    println!("        {:<11} {}{code}", check.state, check.command);
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn decide(id: i64, answer: String, note: Option<String>, json: bool) -> Result<ExitCode> {
    let Response::Decided { decision, delivery } = client::call(&Paths::from_env()?, &Request::Decide { id, answer, note })? else {
        bail!("unexpected reply from the daemon");
    };
    if json {
        println!("{}", serde_json::json!({ "decision": decision, "delivery": delivery }));
    } else {
        println!("decision {}: {} ({}): {delivery}", decision.id, decision.answer.as_deref().unwrap_or("?"), decision.summary);
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
