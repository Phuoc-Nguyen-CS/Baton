//! `baton daemon`: owns Baton's state and serves the CLI, TUI and hooks over a
//! private Unix socket. Clients come and go without affecting work (PLAN.md §4).

use std::fs::{self, File, OpenOptions, Permissions, TryLockError};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;

use crate::backend::Backend;
use crate::backend::claude::Claude;
use crate::backend::fake::FakeBackend;
use crate::git;
use crate::paths::Paths;
use crate::protocol::{MAX_MESSAGE, Request, Response};
use crate::review;
use crate::store::{NewTask, Store};
use crate::usage;
use crate::permission::{self, Waiters};
use crate::worker::{self, Ctx};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum BackendKind {
    Claude,
    /// In-memory stand-in that starts no real sessions (tests and demos)
    Fake,
}

pub fn run(paths: &Paths, kind: BackendKind) -> Result<()> {
    fs::create_dir_all(&paths.home)
        .with_context(|| format!("creating {}", paths.home.display()))?;
    fs::set_permissions(&paths.home, Permissions::from_mode(0o700))?;
    let _lock = lock(paths)?;
    let (backend, backend_name): (Arc<dyn Backend>, &'static str) = match kind {
        BackendKind::Claude => (Arc::new(Claude), "claude"),
        BackendKind::Fake => (Arc::new(FakeBackend::new()), "fake"),
    };
    // Telemetry is optional: without the receiver, usage shows as unknown.
    let port = std::env::var("BATON_OTLP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(usage::DEFAULT_PORT);
    let receiver = std::net::TcpListener::bind(("127.0.0.1", port))
        .and_then(|l| l.set_nonblocking(true).map(|()| l))
        .inspect_err(|e| eprintln!("baton daemon: no telemetry receiver on 127.0.0.1:{port} ({e}); usage will show as unknown"))
        .ok();
    let otlp_endpoint = match &receiver {
        Some(l) => Some(format!("http://{}", l.local_addr()?)),
        None => None,
    };
    let ctx = Arc::new(Ctx {
        paths: paths.clone(),
        store: Mutex::new(Store::open(&paths.db())?),
        backend,
        backend_name,
        exe: std::env::current_exe()?,
        waiters: Waiters::default(),
        permission_wait: permission::WAIT,
        otlp_endpoint,
    });
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        if let Some(listener) = receiver {
            tokio::spawn(usage::serve(tokio::net::TcpListener::from_std(listener)?, ctx.clone()));
        }
        serve(ctx).await
    })
}

/// Held for the daemon's lifetime, so only one daemon owns a state directory.
fn lock(paths: &Paths) -> Result<File> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.lock())?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            bail!("another baton daemon is already running for {}", paths.home.display())
        }
        Err(TryLockError::Error(e)) => return Err(e.into()),
    }
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(file)
}

async fn serve(ctx: Arc<Ctx>) -> Result<()> {
    let socket = ctx.paths.socket();
    // We hold the lock, so a socket file left here belongs to a daemon that died.
    remove_if_present(&socket)?;
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    eprintln!(
        "baton daemon {} (pid {}, {} backend) listening on {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        ctx.backend_name,
        socket.display()
    );
    let wake = Arc::new(Notify::new());
    tokio::spawn(schedule(ctx.clone(), wake.clone()));
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let (ctx, wake) = (ctx.clone(), wake.clone());
                    tokio::spawn(async move {
                        if let Err(e) = connection(stream, ctx, wake).await {
                            eprintln!("baton daemon: connection: {e:#}");
                        }
                    });
                }
                Err(e) => eprintln!("baton daemon: accept: {e}"),
            },
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }
    remove_if_present(&socket)?;
    eprintln!("baton daemon stopped");
    Ok(())
}

/// Runs the worker lifecycle every poll interval, and at once when woken.
async fn schedule(ctx: Arc<Ctx>, wake: Arc<Notify>) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = wake.notified() => {}
        }
        let c = ctx.clone();
        match tokio::task::spawn_blocking(move || worker::tick(&c)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("baton daemon: {e:#}"),
            Err(e) => eprintln!("baton daemon: scheduler failed: {e}"),
        }
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

async fn connection(stream: UnixStream, ctx: Arc<Ctx>, wake: Arc<Notify>) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(MAX_MESSAGE as u64 + 1));
    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .context("request timed out")??;
    let response = match parse(&line) {
        Ok(request) => tokio::task::spawn_blocking(move || handle(&ctx, &wake, request)).await?,
        Err(message) => Response::Error { message },
    };
    let mut out = serde_json::to_vec(&response)?;
    out.push(b'\n');
    write.write_all(&out).await?;
    Ok(())
}

fn parse(line: &str) -> Result<Request, String> {
    if !line.ends_with('\n') {
        return Err(if line.len() > MAX_MESSAGE {
            format!("request exceeds {MAX_MESSAGE} bytes")
        } else {
            "request must end with a newline".into()
        });
    }
    serde_json::from_str(line).map_err(|e| format!("bad request: {e}"))
}

fn handle(ctx: &Ctx, wake: &Notify, request: Request) -> Response {
    let result = match request {
        Request::Ping => Ok(Response::Pong {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
        }),
        Request::Status => {
            let store = ctx.store.lock().unwrap();
            store.task_views().and_then(|tasks| Ok(Response::Status { tasks, quota: store.quota()? }))
        }
        Request::CreateTask { request_id, repo, goal, checks, model } => {
            create_task(ctx, request_id, &repo, goal, checks, model).inspect(|r| {
                if matches!(r, Response::Task { created: true, .. }) {
                    wake.notify_one();
                }
            })
        }
        Request::Hook { attempt, event, input, denied } => {
            worker::on_hook(ctx, attempt, &event, &input, denied.as_deref()).map(|output| Response::Hook { output })
        }
        Request::Decide { id, answer, note } => permission::decide(ctx, id, &answer, note.as_deref())
            .map(|(decision, delivery)| Response::Decided { decision, delivery: delivery.into() }),
        Request::Detail { task } => review::detail(ctx, task).map(|detail| Response::Detail { detail }),
        Request::Review { task, verdict, candidate, note } => {
            review::review(ctx, task, verdict, candidate.as_deref(), note.as_deref())
                .map(|(task, outcome)| Response::Reviewed { task, outcome })
        }
    };
    result.unwrap_or_else(|e| Response::Error { message: format!("{e:#}") })
}

fn create_task(
    ctx: &Ctx,
    request_id: String,
    repo: &Path,
    goal: String,
    checks: Vec<String>,
    model: Option<String>,
) -> Result<Response> {
    if goal.trim().is_empty() {
        bail!("the goal is empty");
    }
    if !repo.is_absolute() {
        bail!("repo path must be absolute: {}", repo.display());
    }
    let r = git::repo(repo).with_context(|| format!("{} isn't a usable git repository", repo.display()))?;
    let new = NewTask { request_id, repo: r.main, base_rev: r.head, goal, checks, model };
    let (task, created) = ctx.store.lock().unwrap().create_task(&new)?;
    Ok(Response::Task { task, created })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_must_be_one_bounded_line() {
        assert!(matches!(parse("{\"op\":\"ping\"}\n"), Ok(Request::Ping)));
        assert_eq!(parse("{\"op\":\"ping\"}").unwrap_err(), "request must end with a newline");
        let huge = "x".repeat(MAX_MESSAGE + 1);
        assert!(parse(&huge).unwrap_err().contains("exceeds"));
        assert!(parse("{\"op\":\"launch\"}\n").unwrap_err().starts_with("bad request"));
    }
}
