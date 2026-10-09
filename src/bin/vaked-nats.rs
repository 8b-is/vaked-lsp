// vaked-nats — the actor-mesh EventBus sidecar.
//
// One MCP endpoint in front of NATS: the mesh's nervous system. Every
// actor is a name; every name is a subject; every subject is a route.
// This sidecar is the mailbox/registry the agents and the umbrella talk
// to — publish, request/reply, and observe the actor subjects.
//
// MCP protocol over stdio, same framing as vaked-mcp:
//   initialize · tools/list · tools/call · shutdown · exit

use std::io::{Read, Write};
use std::sync::OnceLock;
use futures::StreamExt;
use vaked_lsp::frame::{encode_frame_bytes, FrameReader};

// one multi-threaded runtime for the whole process — NATS connections
// keep their background tasks alive on it (a per-call runtime dies with it).
fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

struct Config {
    nats_url: String,
    nats_token: Option<String>,
}

impl Config {
    fn from_env() -> Self {
        Self {
            nats_url: env("VAKED_NATS_URL", "nats://127.0.0.1:4222"),
            nats_token: std::env::var("VAKED_NATS_TOKEN").ok(),
        }
    }
}

fn lane(name: &str, ok: bool, detail: String) -> serde_json::Value {
    serde_json::json!({ "lane": name, "ok": ok, "detail": detail })
}

// each call gets a short-lived connection (NATS connects in ms)
fn connect(cfg: &Config) -> Result<async_nats::Client, String> {
    let mut opts = async_nats::ConnectOptions::new();
    if let Some(tok) = &cfg.nats_token {
        opts = opts.token(tok.clone());
    }
    rt().block_on(async {
        async_nats::connect_with_options(&cfg.nats_url, opts)
            .await
            .map_err(|e| format!("connect failed: {e}"))
    })
}

// ── tools ────────────────────────────────────────────────────────────────
fn tool_nats_status(cfg: &Config) -> serde_json::Value {
    match connect(cfg) {
        Ok(_c) => lane("nats", true, format!("connected to {}", cfg.nats_url)),
        Err(e) => lane("nats", false, e),
    }
}

fn tool_nats_publish(cfg: &Config, subject: &str, payload: &str) -> serde_json::Value {
    if subject.is_empty() {
        return lane("nats.publish", false, "subject required".into());
    }
    let c = match connect(cfg) {
        Ok(c) => c,
        Err(e) => return lane("nats.publish", false, e),
    };
    let bytes = payload.as_bytes().to_vec();
    // publish on the shared multi-thread runtime — a per-call runtime was
    // ~ms of startup on the mesh's hottest path
    match rt().block_on(c.publish(subject.to_string(), bytes.into())) {
        Ok(_) => lane("nats.publish", true, format!("published {subject} · {}B", payload.len())),
        Err(e) => lane("nats.publish", false, format!("{e}")),
    }
}

async fn request_with_deadline<T, E: std::fmt::Display>(
    timeout_ms: u64,
    request: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), request).await {
        Ok(result) => result.map_err(|e| format!("request failed: {e}")),
        Err(_) => Err(format!("request timed out after {timeout_ms} ms")),
    }
}

fn tool_nats_request(cfg: &Config, subject: &str, payload: &str, timeout_ms: u64) -> serde_json::Value {
    if subject.is_empty() {
        return lane("nats.request", false, "subject required".into());
    }
    let c = match connect(cfg) {
        Ok(c) => c,
        Err(e) => return lane("nats.request", false, e),
    };

    let fut = async {
        // The outer deadline includes enqueueing. Disable the client's default
        // response timeout so it cannot silently shorten the caller's budget.
        let request = async_nats::Request::new()
            .payload(payload.as_bytes().to_vec().into())
            .timeout(None);
        let msg = request_with_deadline(timeout_ms, c.send_request(subject.to_string(), request)).await?;
        Ok(String::from_utf8_lossy(&msg.payload).to_string())
    };
    match rt().block_on(fut) {
        Ok(reply) => lane("nats.request", true, format!("{subject} → {reply}")),
        Err(e) => lane("nats.request", false, e),
    }
}

fn tool_nats_subscribe(cfg: &Config, subject: &str, n: u64, timeout_ms: u64) -> serde_json::Value {
    if subject.is_empty() {
        return lane("nats.subscribe", false, "subject required".into());
    }
    let c = match connect(cfg) {
        Ok(c) => c,
        Err(e) => return lane("nats.subscribe", false, e),
    };

    let fut = async {
        let mut sub = c.subscribe(subject.to_string()).await.map_err(|e| format!("subscribe failed: {e}"))?;
        let mut got = Vec::new();
        for _ in 0..n {
            match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), sub.next()).await {
                Ok(Some(msg)) => got.push(String::from_utf8_lossy(&msg.payload).to_string()),
                Ok(None) => break,
                Err(_) => break, // timeout waiting
            }
        }
        Ok(got.join(" | "))
    };
    match rt().block_on(fut) {
        Ok(msgs) => lane("nats.subscribe", true, format!("{subject} → {msgs}")),
        Err(e) => lane("nats.subscribe", false, e),
    }
}

// ── tool registry ────────────────────────────────────────────────────────
fn tools_list() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"name":"nats_status","description":"is the actor-mesh reachable?","inputSchema":{"type":"object","properties":{}}}),
        serde_json::json!({"name":"nats_publish","description":"publish a message to an actor subject (the mailbox)","inputSchema":{"type":"object","properties":{"subject":{"type":"string"},"payload":{"type":"string"}},"required":["subject"]}}),
        serde_json::json!({"name":"nats_request","description":"request/reply on a subject (the reducer call)","inputSchema":{"type":"object","properties":{"subject":{"type":"string"},"payload":{"type":"string"},"timeout_ms":{"type":"integer"}},"required":["subject"]}}),
        serde_json::json!({"name":"nats_subscribe","description":"observe N messages on a subject (the witness)","inputSchema":{"type":"object","properties":{"subject":{"type":"string"},"n":{"type":"integer"},"timeout_ms":{"type":"integer"}},"required":["subject"]}}),
    ]
}

fn dispatch(cfg: &Config, name: &str, args: &serde_json::Value) -> serde_json::Value {
    match name {
        "nats_status" => tool_nats_status(cfg),
        "nats_publish" => {
            let subject = args.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let payload = args.get("payload").and_then(|v| v.as_str()).unwrap_or("");
            tool_nats_publish(cfg, subject, payload)
        }
        "nats_request" => {
            let subject = args.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let payload = args.get("payload").and_then(|v| v.as_str()).unwrap_or("");
            let timeout = args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(2000);
            tool_nats_request(cfg, subject, payload, timeout)
        }
        "nats_subscribe" => {
            let subject = args.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            let n = args.get("n").and_then(|v| v.as_u64()).unwrap_or(1);
            let timeout = args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(3000);
            tool_nats_subscribe(cfg, subject, n, timeout)
        }
        other => lane("unknown", false, format!("no tool named {other} — the mesh knows: nats_status, nats_publish, nats_request, nats_subscribe")),
    }
}

// ── the MCP loop ─────────────────────────────────────────────────────────
fn main() {
    let cfg = Config::from_env();
    let mut reader = FrameReader::new(std::io::stdin().lock());
    let mut stdout = std::io::stdout().lock();
    let mut initialized = false;
    let mut running = true;

    while running {
        let frame = match reader.read_frame() {
            Ok(f) => f,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    return;
                }
                eprintln!("[vaked-nats] frame error: {e}");
                continue;
            }
        };
        let msg: serde_json::Value = match serde_json::from_str(&frame) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(serde_json::Value::Null);

        if id.is_some() {
            let (result, err): (Option<serde_json::Value>, Option<serde_json::Value>) = match method {
                "initialize" => {
                    initialized = true;
                    (
                        Some(serde_json::json!({
                            "protocolVersion": "2025-11-25",
                            "capabilities": { "tools": { "listChanged": false } },
                            "serverInfo": {
                                "name": "vaked-nats",
                                "title": "vaked-nats · the actor-mesh EventBus",
                                "version": env!("CARGO_PKG_VERSION"),
                                "description": "the NATS actor-mesh sidecar: publish, request/reply, subscribe — every actor is a subject, every subject is a route"
                            },
                            "instructions": "use nats_status to probe the mesh, nats_publish to send to an actor subject, nats_request for reducer-style calls, nats_subscribe to witness N messages"
                        })),
                        None,
                    )
                }
                "tools/list" => (Some(serde_json::json!({ "tools": tools_list() })), None),
                "tools/call" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = params.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
                    let out = dispatch(&cfg, name, &args);
                    let is_err = out.get("ok").and_then(|v| v.as_bool()) == Some(false);
                    (
                        Some(serde_json::json!({
                            "content": [ { "type": "text", "text": out.to_string() } ],
                            "isError": is_err
                        })),
                        None,
                    )
                }
                "ping" => (Some(serde_json::json!({})), None),
                "shutdown" | "exit" => { running = false; (Some(serde_json::json!({})), None) }
                other => (
                    None,
                    Some(serde_json::json!({ "code": -32601, "message": format!("method not found: {other}") })),
                ),
            };
            let reply = match err {
                Some(e) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": e }),
                None => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            };
            if let Ok(bytes) = serde_json::to_vec(&reply) {
                let _ = stdout.write_all(&encode_frame_bytes(&bytes));
                let _ = stdout.flush();
            }
        } else if method == "notifications/initialized" {
            initialized = true;
        }
        // notifications/cancelled and everything else without an id: ignored
        let _ = initialized;
    }
}

#[cfg(test)]
mod tests {
    use super::request_with_deadline;

    #[tokio::test]
    async fn pending_request_obeys_deadline() {
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1),
            request_with_deadline(20, std::future::pending::<Result<(), &str>>())).await;
        assert_eq!(result.expect("caller deadline not applied"), Err("request timed out after 20 ms".into()));
        assert!(start.elapsed() >= std::time::Duration::from_millis(20));
    }

    #[tokio::test]
    async fn fast_reply_and_error_are_preserved() {
        assert_eq!(request_with_deadline(100, async { Ok::<_, &str>("reply") }).await, Ok("reply"));
        assert_eq!(request_with_deadline(100, async { Err::<(), _>("synthetic failure") }).await,
            Err("request failed: synthetic failure".into()));
    }
}
