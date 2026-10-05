//! Blocking client for the daemon socket, for the CLI and hooks.

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::paths::Paths;
use crate::protocol::{MAX_MESSAGE, Request, Response};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Sends one request; a daemon-side error comes back as `Err`.
pub fn call(paths: &Paths, request: &Request) -> Result<Response> {
    call_with_timeout(paths, request, REPLY_TIMEOUT)
}

pub fn call_with_timeout(paths: &Paths, request: &Request, timeout: Duration) -> Result<Response> {
    let socket = paths.socket();
    let mut stream = UnixStream::connect(&socket).map_err(|e| match e.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => anyhow!(
            "the Baton daemon isn't running (no listener on {}); start it with `baton daemon`",
            socket.display()
        ),
        _ => anyhow!(e).context(format!("connecting to {}", socket.display())),
    })?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line)?;

    let mut reply = String::new();
    BufReader::new(stream.take(MAX_MESSAGE as u64 + 1))
        .read_line(&mut reply)
        .context("reading the daemon's reply")?;
    if !reply.ends_with('\n') {
        bail!("the daemon closed the connection without a complete reply");
    }
    match serde_json::from_str(&reply)? {
        Response::Error { message } => bail!(message),
        response => Ok(response),
    }
}

/// A fresh random id for a request that should take effect at most once.
pub fn new_request_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
