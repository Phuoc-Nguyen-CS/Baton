//! Usage accounting (D6, compat-record C12): an OTLP/HTTP-JSON receiver for
//! workers' `api_request` events, and the transcript cross-check. Only numbers,
//! the model and the session id are kept; every event also carries the owner's
//! email and account ids, which are never stored.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::model::Tokens;
use crate::worker::Ctx;

/// The receiver's default port; `BATON_OTLP_PORT` overrides it (0 = any free port).
pub const DEFAULT_PORT: u16 = 47318;

const MAX_HEAD: usize = 16 << 10;
const MAX_BODY: usize = 8 << 20;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// One API request as reported by Claude Code.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    /// The session UUID (`session.id`).
    pub session: String,
    /// Unique per request, so a replayed export counts once.
    pub key: String,
    pub model: Option<String>,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub at_ms: i64,
}

/// The `api_request` events in an OTLP logs export.
pub fn api_requests(body: &Value) -> Vec<ApiRequest> {
    let mut out = Vec::new();
    let records = body["resourceLogs"].as_array().into_iter().flatten().flat_map(|rl| {
        rl["scopeLogs"].as_array().into_iter().flatten().flat_map(|sl| sl["logRecords"].as_array().into_iter().flatten())
    });
    for record in records {
        let attrs = record["attributes"].as_array();
        let attr = |key: &str| attrs.into_iter().flatten().find(|a| a["key"] == key).map(|a| &a["value"]);
        let text = |key: &str| attr(key).and_then(|v| v["stringValue"].as_str()).map(str::to_owned);
        let num = |key: &str| attr(key).and_then(number).unwrap_or(0.0);
        if text("event.name").as_deref() != Some("api_request") {
            continue;
        }
        let Some(session) = text("session.id") else { continue };
        let key = text("request_id").unwrap_or_else(|| {
            format!("{session}:{}:{}", text("event.timestamp").unwrap_or_default(), num("event.sequence"))
        });
        let at_ms = record["timeUnixNano"].as_str().and_then(|n| n.parse::<i64>().ok()).map_or(0, |n| n / 1_000_000);
        out.push(ApiRequest {
            session,
            key,
            model: text("model"),
            tokens: Tokens {
                input: num("input_tokens") as i64,
                output: num("output_tokens") as i64,
                cache_read: num("cache_read_tokens") as i64,
                cache_write: num("cache_creation_tokens") as i64,
            },
            cost_usd: num("cost_usd"),
            at_ms,
        });
    }
    out
}

/// An OTLP JSON value as a number; integers may arrive as numbers or strings.
fn number(v: &Value) -> Option<f64> {
    for kind in ["intValue", "doubleValue", "stringValue"] {
        match &v[kind] {
            Value::Number(n) => return n.as_f64(),
            Value::String(s) => return s.parse().ok(),
            _ => {}
        }
    }
    None
}

/// Token totals in a transcript: assistant entries with `usage`, one per message
/// id (a message spans several entries; the last one counts). The format is
/// undocumented, so this is a fallible cross-check, never the source of truth.
pub fn transcript_tokens(path: &Path) -> Result<Tokens> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut by_message: Vec<(String, Tokens)> = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        let message = &entry["message"];
        let usage = &message["usage"];
        if entry["type"] != "assistant" || !usage.is_object() {
            continue;
        }
        let id = message["id"].as_str().unwrap_or_default().to_owned();
        let count = |k: &str| usage[k].as_i64().unwrap_or(0);
        let tokens = Tokens {
            input: count("input_tokens"),
            output: count("output_tokens"),
            cache_read: count("cache_read_input_tokens"),
            cache_write: count("cache_creation_input_tokens"),
        };
        match by_message.iter_mut().find(|(m, _)| *m == id) {
            Some(slot) => slot.1 = tokens,
            None => by_message.push((id, tokens)),
        }
    }
    let mut total = Tokens::default();
    for (_, t) in by_message {
        total += t;
    }
    Ok(total)
}

/// Serves OTLP/HTTP-JSON exports from workers until the daemon stops.
pub async fn serve(listener: TcpListener, ctx: Arc<Ctx>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(e) = connection(stream, ctx).await {
                        eprintln!("baton daemon: telemetry: {e:#}");
                    }
                });
            }
            Err(e) => eprintln!("baton daemon: telemetry accept: {e}"),
        }
    }
}

/// One request per connection: `POST /v1/logs` with a `Content-Length` body.
/// Other `/v1/` signals are accepted and ignored.
async fn connection(mut stream: TcpStream, ctx: Arc<Ctx>) -> Result<()> {
    let status = match tokio::time::timeout(READ_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(Read::Body(path, body))) if path == "/v1/logs" => match serde_json::from_slice::<Value>(&body) {
            Ok(export) => {
                let requests = api_requests(&export);
                tokio::task::spawn_blocking(move || {
                    let mut store = ctx.store.lock().unwrap();
                    requests.iter().try_for_each(|r| store.record_usage(r).map(drop))
                })
                .await??;
                "200 OK"
            }
            Err(_) => "400 Bad Request",
        },
        Ok(Ok(Read::Body(..))) => "200 OK",
        Ok(Ok(Read::Refused(status))) => status,
        Ok(Err(e)) => {
            eprintln!("baton daemon: telemetry: {e:#}");
            "400 Bad Request"
        }
        Err(_) => "408 Request Timeout",
    };
    let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
    stream.write_all(reply.as_bytes()).await?;
    Ok(())
}

enum Read {
    Body(String, Vec<u8>),
    Refused(&'static str),
}

async fn read_request(stream: &mut TcpStream) -> Result<Read> {
    let mut reader = BufReader::new(stream);
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if reader.read_until(b'\n', &mut head).await? == 0 || head.len() > MAX_HEAD {
            bail!("incomplete or oversized request head");
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let (method, path) = (first.next().unwrap_or_default(), first.next().unwrap_or_default().to_owned());
    let mut length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse::<usize>().ok(),
                "transfer-encoding" => return Ok(Read::Refused("411 Length Required")),
                _ => {}
            }
        }
    }
    let Some(length) = length else { return Ok(Read::Refused("411 Length Required")) };
    if length > MAX_BODY {
        return Ok(Read::Refused("413 Payload Too Large"));
    }
    if method != "POST" || !path.starts_with("/v1/") {
        return Ok(Read::Refused("404 Not Found"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(Read::Body(path, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn export(records: Vec<Value>) -> Value {
        json!({ "resourceLogs": [{ "resource": { "attributes": [] }, "scopeLogs": [{ "logRecords": records }] }] })
    }

    fn record(attrs: &[(&str, Value)]) -> Value {
        let attributes: Vec<Value> = attrs.iter().map(|(k, v)| json!({ "key": k, "value": v })).collect();
        json!({ "timeUnixNano": "1791152849189000000", "attributes": attributes })
    }

    #[test]
    fn api_requests_keep_numbers_and_drop_identity() {
        let body = export(vec![
            record(&[
                ("user.email", json!({ "stringValue": "owner@example.com" })),
                ("session.id", json!({ "stringValue": "401ee679-6f74" })),
                ("event.name", json!({ "stringValue": "api_request" })),
                ("model", json!({ "stringValue": "claude-haiku-4-5-20251001" })),
                ("input_tokens", json!({ "intValue": 10 })),
                ("output_tokens", json!({ "intValue": "209" })),
                ("cache_read_tokens", json!({ "intValue": 24595 })),
                ("cache_creation_tokens", json!({ "intValue": 9277 })),
                ("cost_usd", json!({ "doubleValue": 0.0220685 })),
                ("request_id", json!({ "stringValue": "req_1" })),
            ]),
            record(&[("event.name", json!({ "stringValue": "tool_result" })), ("session.id", json!({ "stringValue": "x" }))]),
        ]);
        let requests = api_requests(&body);
        assert_eq!(
            requests,
            [ApiRequest {
                session: "401ee679-6f74".into(),
                key: "req_1".into(),
                model: Some("claude-haiku-4-5-20251001".into()),
                tokens: Tokens { input: 10, output: 209, cache_read: 24595, cache_write: 9277 },
                cost_usd: 0.0220685,
                at_ms: 1791152849189,
            }]
        );
        assert!(!format!("{requests:?}").contains("owner@example.com"));
    }

    #[test]
    fn transcript_counts_each_message_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let entry = |id: &str, out: i64| {
            json!({ "type": "assistant", "message": { "id": id, "usage": { "input_tokens": 3, "output_tokens": out, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 7 } } })
        };
        let lines = [entry("m1", 1), entry("m1", 5), json!({ "type": "user", "message": {} }), entry("m2", 2)];
        let text: Vec<String> = lines.iter().map(Value::to_string).chain(["not json".into()]).collect();
        std::fs::write(&path, text.join("\n")).unwrap();
        assert_eq!(transcript_tokens(&path).unwrap(), Tokens { input: 6, output: 7, cache_read: 200, cache_write: 14 });
        assert!(transcript_tokens(&dir.path().join("missing")).is_err());
    }
}
