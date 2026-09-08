// vaked-mcp — the umbrella MCP sidecar of vaked-lsp.
//
// One MCP endpoint in front of the engines: Unreal, Unity, FAB, and the
// toolchain around them. Runs as a SIDECAR of the LSP gateway — same
// crate, same framing (Content-Length JSON-RPC), same doctrine: one door,
// many lanes; the lane stays dark when its engine is not running.
//
//   vaked-lsp  (stdio)  →  editors see languages
//   vaked-mcp  (stdio)  →  agents see engines
//
// MCP protocol over stdio, minimal surface:
//   initialize · notifications/initialized · tools/list · tools/call ·
//   notifications/cancelled (ignored) · shutdown · exit

use std::io::{Read, Write};
use std::process::Command;
use vaked_lsp::frame::{encode_frame, read_frame};

// ── configuration (env or defaults) ────────────────────────────────────────
fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

struct Config {
    ue_cmd: String,      // path to UnrealEditor-Cmd
    unity_cmd: String,   // path to Unity editor binary
    fab_base: String,    // fab search base url
    unity_cloud_dir: String, // uv project holding the Unity Cloud Python SDK lane
    mlx_sidecar_dir: String, // uv project holding the MLX coder sidecar
}

impl Config {
    fn from_env() -> Self {
        Self {
            ue_cmd: env("VAKED_UE_CMD", "UnrealEditor-Cmd"),
            unity_cmd: env("VAKED_UNITY", "unity-editor"),
            fab_base: env("VAKED_FAB_BASE", "https://www.fab.com/en-US/search"),
            unity_cloud_dir: env("VAKED_UNITY_CLOUD_DIR", "unity-cloud"),
            mlx_sidecar_dir: env("VAKED_MLX_SIDECAR_DIR", "mlx-sidecar"),
        }
    }
    fn ue_warm(&self) -> bool { std::path::Path::new(&self.ue_cmd).exists() }
    fn unity_warm(&self) -> bool { std::path::Path::new(&self.unity_cmd).exists() }
}

// ── the lanes ──────────────────────────────────────────────────────────────
fn lane(name: &str, ok: bool, detail: String) -> serde_json::Value {
    serde_json::json!({ "lane": name, "ok": ok, "detail": detail })
}

fn run_capture(program: &str, args: &[&str]) -> Result<String, String> {
    match Command::new(program).args(args).output() {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if !o.status.success() {
                Err(format!("exit {} · {err}", o.status.code().unwrap_or(-1)))
            } else if !out.is_empty() {
                Ok(out)
            } else {
                Ok("ok".to_string())
            }
        }
        Err(e) => Err(format!("spawn failed: {e}")),
    }
}

// ── tool implementations ───────────────────────────────────────────────────
fn tool_umbrella_status(cfg: &Config) -> serde_json::Value {
    serde_json::json!({
        "umbrella": "vaked-mcp · sidecar of vaked-lsp",
        "lanes": {
            "unreal":  { "cmd": cfg.ue_cmd, "warm": cfg.ue_warm() },
            "unity":   { "cmd": cfg.unity_cmd, "warm": cfg.unity_warm() },
            "unity-cloud": { "uv_project": cfg.unity_cloud_dir, "warm": std::path::Path::new(&cfg.unity_cloud_dir).join("cli.py").exists() },
            "mlx":        { "uv_project": cfg.mlx_sidecar_dir, "warm": std::path::Path::new(&cfg.mlx_sidecar_dir).join("sidecar.py").exists() },
            "fab":     { "base": cfg.fab_base },
        },
        "doctrine": "one door, many lanes — the lane stays dark when its engine is not running"
    })
}

fn tool_ue_status(cfg: &Config) -> serde_json::Value {
    if cfg.ue_warm() {
        lane("unreal", true, format!("UnrealEditor-Cmd at {}", cfg.ue_cmd))
    } else {
        lane("unreal", false, format!("{} not found — set VAKED_UE_CMD", cfg.ue_cmd))
    }
}

// ue_console — run a UE console command in batch mode: generate the
// UnrealEditor-Cmd invocation; execute it when the binary is warm.
fn tool_ue_console(cfg: &Config, command: &str, project: Option<&str>) -> serde_json::Value {
    let p = project.unwrap_or("YourGame.uproject");
    let cmdline = format!(
        "{} {} -ExecCmds=\"{}\" -unattended -nop4 -nosplash",
        cfg.ue_cmd, p, command
    );
    if cfg.ue_warm() {
        match Command::new(&cfg.ue_cmd)
            .arg(p)
            .arg(format!("-ExecCmds={command}"))
            .arg("-unattended").arg("-nop4").arg("-nosplash")
            .output()
        {
            Ok(o) => lane("unreal.console", o.status.success(),
                format!("exit {} · {}", o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).trim())),
            Err(e) => lane("unreal.console", false, format!("spawn failed: {e}")),
        }
    } else {
        // the lane stays dark — the agent still gets the key, not a dead end
        lane("unreal.console", false, format!("binary not warm — run manually: {cmdline}"))
    }
}

// ue_python — run a Python script through the engine (batch).
fn tool_ue_python(cfg: &Config, script: &str, project: Option<&str>) -> serde_json::Value {
    let p = project.unwrap_or("YourGame.uproject");
    let cmdline = format!("{} {} -RunPythonScript=\"{}\" -unattended -nop4", cfg.ue_cmd, p, script);
    if cfg.ue_warm() {
        match Command::new(&cfg.ue_cmd)
            .arg(p).arg(format!("-RunPythonScript={script}")).arg("-unattended").arg("-nop4")
            .output()
        {
            Ok(o) => lane("unreal.python", o.status.success(),
                format!("exit {} · {}", o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).trim())),
            Err(e) => lane("unreal.python", false, format!("spawn failed: {e}")),
        }
    } else {
        lane("unreal.python", false, format!("binary not warm — run manually: {cmdline}"))
    }
}

// unity_batch — run an Editor method in batch mode.
fn tool_unity_batch(cfg: &Config, method: &str) -> serde_json::Value {
    let cmdline = format!("{} -batchmode -quit -projectPath . -executeMethod {}", cfg.unity_cmd, method);
    if cfg.unity_warm() {
        match Command::new(&cfg.unity_cmd)
            .arg("-batchmode").arg("-quit").arg("-projectPath").arg(".")
            .arg("-executeMethod").arg(method)
            .output()
        {
            Ok(o) => lane("unity.batch", o.status.success(),
                format!("exit {} · {}", o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).trim())),
            Err(e) => lane("unity.batch", false, format!("spawn failed: {e}")),
        }
    } else {
        lane("unity.batch", false, format!("editor not warm — run manually: {cmdline}"))
    }
}

// unity_peek — read-only local inspection of a Unity scene asset (works
// with no editor running: the lane is a file, not a process).
fn tool_unity_peek(asset: &str) -> serde_json::Value {
    match std::fs::read_to_string(asset) {
        Ok(text) => {
            let gameobjects = text.matches("--- !u!1 &").count();
            let names: Vec<String> = text.lines()
                .filter(|l| l.trim_start().starts_with("m_Name:"))
                .map(|l| l.trim().to_string())
                .take(6).collect();
            lane("unity.asset", true, format!("{gameobjects} GameObjects · {names:?} (peek of {asset})"))
        }
        Err(e) => lane("unity.asset", false, format!("unreadable {asset}: {e}")),
    }
}

// mlx_* — the MLX coder sidecar lane: fast top-SWE-score coders, local.
// Dispatches to the uv project (vaked-lsp/mlx-sidecar).
fn tool_mlx(cfg: &Config, action: &str, args: &serde_json::Value) -> serde_json::Value {
    let mut cmd = Command::new("uv");
    cmd.arg("run").arg("--project").arg(&cfg.mlx_sidecar_dir)
        .arg("python").arg(format!("{}/sidecar.py", cfg.mlx_sidecar_dir)).arg(action);
    if let Some(v) = args.get("model").and_then(|v| v.as_str()) {
        if !v.is_empty() { cmd.arg(v); }
    }
    if args.get("quantized").and_then(|v| v.as_bool()).unwrap_or(false) {
        cmd.arg("--quantized");
    }
    match cmd.output() {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let detail = if !out.is_empty() { out } else { err };
            lane("mlx", o.status.success(),
                format!("exit {} · {}", o.status.code().unwrap_or(-1), detail))
        }
        Err(e) => lane("mlx", false, format!("spawn failed: {e}")),
    }
}

// fab_search — the asset marketplace lane: returns the browse/search URL.
fn tool_fab_search(cfg: &Config, query: &str) -> serde_json::Value {
    let url = format!("{}?keywords={}", cfg.fab_base, query.replace(' ', "%20"));
    lane("fab", true, format!("open in a browser or QWave: {url}"))
}

// unity_cloud — the Unity Cloud Asset Manager lane: dispatches to the Python
// SDK CLI (vaked-lsp/unity-cloud, the uv project). Actions: projects, assets,
// search, datasets, upload, download, whoami. The lane stays dark when the
// uv project is missing.
fn tool_unity_cloud(cfg: &Config, action: &str, args: &serde_json::Value) -> serde_json::Value {
    let mut cmd = Command::new("uv");
    cmd.arg("run").arg("--project").arg(&cfg.unity_cloud_dir)
        .arg("python").arg(format!("{}/cli.py", cfg.unity_cloud_dir)).arg(action);
    for (flag, key) in [
        ("--org", "org"), ("--project", "project"), ("--asset", "asset"),
        ("--version", "version"), ("--dataset", "dataset"),
        ("--file", "file"), ("--out", "out"),
    ] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() { cmd.arg(flag).arg(v); }
        }
    }
    if args.get("service_account").and_then(|v| v.as_bool()).unwrap_or(false) {
        cmd.arg("--service-account");
    }
    match cmd.output() {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            let detail = if !out.is_empty() { out } else { err };
            lane("unity.cloud", o.status.success(),
                format!("exit {} · {}", o.status.code().unwrap_or(-1), detail))
        }
        Err(e) => lane("unity.cloud", false, format!("spawn failed: {e}")),
    }
}

// ── tool registry ──────────────────────────────────────────────────────────
fn tools_list() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"name":"umbrella_status","description":"the sidecar's lane map — which engines are warm","inputSchema":{"type":"object","properties":{}}}),
        serde_json::json!({"name":"ue_status","description":"is the Unreal toolchain warm?","inputSchema":{"type":"object","properties":{}}}),
        serde_json::json!({"name":"ue_console","description":"run a UE console command in batch mode","inputSchema":{"type":"object","properties":{"command":{"type":"string"},"project":{"type":"string"}},"required":["command"]}}),
        serde_json::json!({"name":"ue_python","description":"run a Python script through the engine (batch)","inputSchema":{"type":"object","properties":{"script":{"type":"string"},"project":{"type":"string"}},"required":["script"]}}),
        serde_json::json!({"name":"unity_batch","description":"run a Unity Editor method in batch mode","inputSchema":{"type":"object","properties":{"method":{"type":"string"}},"required":["method"]}}),
        serde_json::json!({"name":"unity_peek","description":"inspect a Unity scene asset locally (no editor needed)","inputSchema":{"type":"object","properties":{"asset":{"type":"string"}},"required":["asset"]}}),
        serde_json::json!({"name":"unity_cloud","description":"the Unity Cloud Asset Manager lane (Python SDK): projects, assets, search, datasets, upload, download, whoami","inputSchema":{"type":"object","properties":{"action":{"type":"string","enum":["projects","assets","search","datasets","upload","download","whoami"]},"org":{"type":"string"},"project":{"type":"string"},"asset":{"type":"string"},"version":{"type":"string"},"dataset":{"type":"string"},"file":{"type":"string"},"out":{"type":"string"},"service_account":{"type":"boolean"}},"required":["action"]}}),
        serde_json::json!({"name":"mlx_status","description":"which local MLX coder lanes are up and healthy","inputSchema":{"type":"object","properties":{}}}),
        serde_json::json!({"name":"mlx_models","description":"the fast top-SWE-score coder catalog (local MLX)","inputSchema":{"type":"object","properties":{}}}),
        serde_json::json!({"name":"mlx_start","description":"start a local MLX coder lane (omit model to auto-select by free memory)","inputSchema":{"type":"object","properties":{"model":{"type":"string"},"quantized":{"type":"boolean"},"force":{"type":"boolean"}}}}),
        serde_json::json!({"name":"mlx_stop","description":"stop a local MLX coder lane","inputSchema":{"type":"object","properties":{"model":{"type":"string"}},"required":["model"]}}),
        serde_json::json!({"name":"fab_search","description":"build a FAB asset marketplace search URL","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}),
    ]
}

fn dispatch(cfg: &Config, name: &str, args: &serde_json::Value) -> serde_json::Value {
    match name {
        "umbrella_status" => tool_umbrella_status(cfg),
        "ue_status" => tool_ue_status(cfg),
        "ue_console" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let project = args.get("project").and_then(|v| v.as_str());
            tool_ue_console(cfg, command, project)
        }
        "ue_python" => {
            let script = args.get("script").and_then(|v| v.as_str()).unwrap_or("");
            let project = args.get("project").and_then(|v| v.as_str());
            tool_ue_python(cfg, script, project)
        }
        "unity_batch" => {
            let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("");
            tool_unity_batch(cfg, method)
        }
        "unity_peek" => {
            let asset = args.get("asset").and_then(|v| v.as_str()).unwrap_or("");
            tool_unity_peek(asset)
        }
        "unity_cloud" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            tool_unity_cloud(cfg, action, args)
        }
        "mlx_status" => tool_mlx(cfg, "status", args),
        "mlx_models" => tool_mlx(cfg, "models", args),
        "mlx_start" => tool_mlx(cfg, "start", args),
        "mlx_stop" => tool_mlx(cfg, "stop", args),
        "fab_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            tool_fab_search(cfg, query)
        }
        other => lane("unknown", false, format!("no tool named {other} — the umbrella knows: umbrella_status, ue_*, unity_*, fab_search")),
    }
}

// ── the MCP loop ───────────────────────────────────────────────────────────
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
                    return; // client closed the pipe — clean exit
                }
                eprintln!("[vaked-mcp] frame error: {e}");
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

        // notifications carry no id — respond only to requests
        if id.is_some() {
            let result = match method {
                "initialize" => {
                    initialized = true;
                    serde_json::json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "vaked-mcp", "version": env!("CARGO_PKG_VERSION") }
                    })
                }
                "tools/list" => serde_json::json!({ "tools": tools_list() }),
                "tools/call" => {
                    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let args = params.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
                    let out = dispatch(&cfg, name, &args).to_string();
                    serde_json::json!({ "content": [ { "type": "text", "text": out } ] })
                }
                "ping" => serde_json::json!({}),
                "shutdown" => { running = false; serde_json::json!({}) }
                "exit" => { running = false; serde_json::json!({}) }
                other => serde_json::json!({ "error": { "code": -32601, "message": format!("method not found: {other}") } }),
            };
            let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id.unwrap(), "result": result });
            let _ = stdout.write_all(&encode_frame(&resp.to_string()));
            let _ = stdout.flush();
        } else if method == "notifications/initialized" {
            initialized = true;
        }
        let _ = initialized;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_respond_with_the_doctrine() {
        let cfg = Config {
            ue_cmd: "/nonexistent/UnrealEditor-Cmd".into(),
            unity_cmd: "/nonexistent/Unity".into(),
            fab_base: "https://www.fab.com/en-US/search".into(),
            unity_cloud_dir: "unity-cloud".into(),
            mlx_sidecar_dir: "mlx-sidecar".into(),
        };
        // cold lanes: never a hard failure — a key, and a door
        let ue = tool_ue_status(&cfg);
        assert_eq!(ue["ok"], false);
        let ue_c = tool_ue_console(&cfg, "stat fps", None);
        assert_eq!(ue_c["lane"], "unreal.console");
        assert!(ue_c["detail"].as_str().unwrap().contains("run manually"));
        // file lanes work with no engine at all
        let peek = tool_unity_peek("Cargo.toml");
        assert_eq!(peek["lane"], "unity.asset");
        // fab is a window, not a process
        let fab = tool_fab_search(&cfg, "neon city");
        assert!(fab["detail"].as_str().unwrap().contains("fab.com"));
    }

    #[test]
    fn dispatch_knows_every_tool() {
        let cfg = Config::from_env();
        let n = tools_list().len();
        for t in tools_list() {
            let name = t["name"].as_str().unwrap();
            let out = dispatch(&cfg, name, &serde_json::json!({}));
            assert!(out["lane"].is_string() || out["lanes"].is_object(), "{name} must answer");
        }
        assert_eq!(n, 12, "the umbrella covers twelve tools");
    }
}