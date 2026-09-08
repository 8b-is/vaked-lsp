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
use vaked_lsp::frame::{encode_frame, read_frame};

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
    // publish on the blocking runtime
    let fut = c.publish(subject.to_string(), bytes.into());
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|rt| rt.block_on(fut))
    {
        Ok(Ok(_)) => lane("nats.publish", true, format!("published {subject} · {}B", payload.len())),
        Ok(Err(e)) => lane("nats.publish", false, format!("{e}")),
        Err(e) => lane("nats.publish", false, format!("{e}")),
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
        let timeout = std::time::Duration::from_millis(timeout_ms);
        match c.request(subject.to_string(), payload.as_bytes().to_vec().into()).await {
            Ok(msg) => {
                let body = String::from_utf8_lossy(&msg.payload).to_string();
                Ok(body)
            }
            Err(e) => Err(format!("request failed: {e}")),
        }
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
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut initialized = false;
    let mut running = true;

    while running {
        let frame = match read_frame(&mut stdin) {
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
            let result = match method {
                "initialize" => {
                    initialized = true;
                    serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "vaked-nats", "version": env!("CARGO_PKG_VERSION") }
                    })
                }
                "tools/list" => serde_json::json!({ "tools": tools_list() }),
                "tools/call" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = params.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
                    let out = dispatch(&cfg, name, &args).to_string();
                    serde_json::json!({ "content": [ { "type": "text", "text": out } ] })
                }
                "shutdown" | "exit" => { running = false; serde_json::json!({}) }
                other => {
                    initialized = false;
                    serde_json::json!({ "error": { "code": -32601, "message": format!("method not found: {other}") } })
                }
            };
            let reply = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let _ = stdout.write_all(&encode_frame(&reply.to_string()));
            let _ = stdout.flush();
        } else if method == "notifications/initialized" {
            initialized = true;
        }
    }
}
