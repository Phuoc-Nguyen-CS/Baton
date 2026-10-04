//! `baton daemon`: owns Baton's state and serves the CLI, TUI and hooks over a
//! private Unix socket. Clients come and go without affecting work (PLAN.md §4).

use std::fs::{self, File, OpenOptions, Permissions, TryLockError};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

use crate::git;
use crate::paths::Paths;
use crate::protocol::{MAX_MESSAGE, Request, Response};
use crate::store::{NewTask, Store};

const READ_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(paths: &Paths) -> Result<()> {
    fs::create_dir_all(&paths.home)
        .with_context(|| format!("creating {}", paths.home.display()))?;
    fs::set_permissions(&paths.home, Permissions::from_mode(0o700))?;
    let _lock = lock(paths)?;
    let store = Store::open(&paths.db())?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(serve(paths, Arc::new(Mutex::new(store))))
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

async fn serve(paths: &Paths, store: Arc<Mutex<Store>>) -> Result<()> {
    let socket = paths.socket();
    // We hold the lock, so a socket file left here belongs to a daemon that died.
    remove_if_present(&socket)?;
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    eprintln!(
        "baton daemon {} (pid {}) listening on {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        socket.display()
    );
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let store = store.clone();
                    tokio::spawn(async move {
                        if let Err(e) = connection(stream, store).await {
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

fn remove_if_present(path: &PathBuf) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

async fn connection(stream: UnixStream, store: Arc<Mutex<Store>>) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(MAX_MESSAGE as u64 + 1));
    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .context("request timed out")??;
    let response = match parse(&line) {
        Ok(request) => tokio::task::spawn_blocking(move || handle(&store, request)).await?,
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

fn handle(store: &Mutex<Store>, request: Request) -> Response {
    let result = match request {
        Request::Ping => Ok(Response::Pong {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
        }),
        Request::Status => store.lock().unwrap().tasks().map(|tasks| Response::Status { tasks }),
        Request::CreateTask { request_id, repo, goal, checks } => {
            create_task(store, request_id, repo, goal, checks)
        }
    };
    result.unwrap_or_else(|e| Response::Error { message: format!("{e:#}") })
}

fn create_task(
    store: &Mutex<Store>,
    request_id: String,
    repo: PathBuf,
    goal: String,
    checks: Vec<String>,
) -> Result<Response> {
    if goal.trim().is_empty() {
        bail!("the goal is empty");
    }
    if !repo.is_absolute() {
        bail!("repo path must be absolute: {}", repo.display());
    }
    let r = git::repo(&repo).with_context(|| format!("{} isn't a usable git repository", repo.display()))?;
    let new = NewTask { request_id, repo: r.main, base_rev: r.head, goal, checks };
    let (task, created) = store.lock().unwrap().create_task(&new)?;
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
