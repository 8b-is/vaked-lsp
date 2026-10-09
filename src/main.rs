// vaked-lsp — the all-in-one LSP gateway.
//
// One LSP endpoint in front of the engine's many languages: UE C++
// (clangd), Rust (rust-analyzer), Go (gopls), Luau (luau-lsp), Bash
// (bash-language-server). Router by file extension; lazy sub-server
// supervision; minimal JSON-RPC framing proxy for the lifecycle +
// completion/hover/definition seam. The constellation's architecture
// at the language level: one door, many lanes.

use vaked_lsp::frame::{content_length, encode_frame_bytes, header_end, jsonrpc_notify, jsonrpc_request, MAX_HEADER_BYTES};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// The routing table — extension → (sub-server command, display name).
/// The wire the constellation runs on: -1/0/+1 for the lane, this table
/// for the door.
fn route_for(uri: &str) -> Option<(&'static str, &'static str)> {
    if uri.ends_with(".cpp") || uri.ends_with(".h") || uri.ends_with(".hpp") || uri.ends_with(".cc") {
        Some(("clangd", "ue-c++"))
    } else if uri.ends_with(".rs") {
        Some(("rust-analyzer", "rust"))
    } else if uri.ends_with(".go") {
        Some(("gopls", "go"))
    } else if uri.ends_with(".luau") || uri.ends_with(".lua") {
        Some(("luau-lsp", "luau"))
    } else if uri.ends_with(".sh") || uri.ends_with(".bash") {
        Some(("bash-language-server", "bash"))
    } else {
        None
    }
}

#[derive(Default)]
struct SubServer {
    child: Option<Child>,
    next_id: u64,
    read_buf: Vec<u8>, // framed-read carry for the sub-server stdout
}

struct RouterState {
    client: Client,
    sub: Arc<Mutex<HashMap<&'static str, SubServer>>>,
}

impl RouterState {
    /// Lazily spawn the sub-server for a language, then forward a raw
    /// JSON-RPC message to it and return the response body.
    async fn forward(&self, uri: &str, method: &str, params: &serde_json::Value) -> Option<serde_json::Value> {
        let (cmd, lane) = route_for(uri)?;
        let mut subs = self.sub.lock().await;
        let entry = subs.entry(lane).or_default();
        if entry.child.is_none() {
            match Command::new(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => {
                    self.client
                        .log_message(MessageType::INFO, format!("[vaked-lsp] {lane} ← {cmd} up")).await;
                    entry.child = Some(child);
                }
                Err(e) => {
                    let _ = self.client.log_message(
                        MessageType::WARNING,
                        format!("[vaked-lsp] {cmd} unavailable ({e}) — the lane stays dark"),
                    ).await;
                    return None;
                }
            }
        }
        let child = entry.child.as_mut()?;
        entry.next_id += 1;
        let id = entry.next_id;
        let notification = matches!(method, "textDocument/didOpen" | "textDocument/didChange");
        let req = if notification {
            jsonrpc_notify(method, params)
        } else {
            jsonrpc_request(id, method, params)
        };
        let req_bytes = serde_json::to_vec(&req).unwrap_or_default();
        // initialize handshake on first forward — the sub-server needs it
        if id == 1 {
            handshake(child, &mut entry.read_buf).await;
        }
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(&encode_frame_bytes(&req_bytes)).await.ok()?;
            stdin.flush().await.ok()?;
        }
        // LSP document notifications carry no id and receive no response.
        if notification {
            return None;
        }
        // read frames until the response carrying our id — notifications
        // ($/progress) and any other ids are skipped, the pipe never desyncs
        if let Some(stdout) = child.stdout.as_mut() {
            for _ in 0..32 {
                let msg = read_sub_frame(stdout, &mut entry.read_buf).await?;
                if msg.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    return Some(msg);
                }
            }
        }
        None
    }
}

/// The LSP initialize handshake on the sub-server: full framed reads (no
/// line-swallowing), and `initialized` as a true notification (no id).
async fn handshake(child: &mut Child, read_buf: &mut Vec<u8>) {
    let init = jsonrpc_request(
        0,
        "initialize",
        &serde_json::json!({
            "processId": null,
            "rootUri": null,
            "capabilities": {}
        }),
    );
    if let Some(stdin) = child.stdin.as_mut() {
        if let Ok(bytes) = serde_json::to_vec(&init) {
            let _ = stdin.write_all(&encode_frame_bytes(&bytes)).await;
            let _ = stdin.flush().await;
        }
    }
    if let Some(stdout) = child.stdout.as_mut() {
        let _ = read_sub_frame(stdout, read_buf).await; // the initialize response
    }
    let inited = jsonrpc_notify("initialized", &serde_json::json!({}));
    if let Some(stdin) = child.stdin.as_mut() {
        if let Ok(bytes) = serde_json::to_vec(&inited) {
            let _ = stdin.write_all(&encode_frame_bytes(&bytes)).await;
            let _ = stdin.flush().await;
        }
    }
}

/// Read one full framed JSON-RPC message from a sub-server's stdout,
/// appending to the per-lane carry buffer — no per-byte syscalls, no
/// String header parsing. The gateway's response path stays cheap.
async fn read_sub_frame(
    stdout: &mut tokio::process::ChildStdout,
    buf: &mut Vec<u8>,
) -> Option<serde_json::Value> {
    loop {
        if let Some(pos) = header_end(buf) {
            let len = content_length(&buf[..pos])?;
            let body_start = pos + 4;
            while buf.len() < body_start + len {
                let n = stdout.read_buf(buf).await.ok()?;
                if n == 0 {
                    return None;
                }
            }
            let v: Option<serde_json::Value> =
                serde_json::from_slice(&buf[body_start..body_start + len]).ok();
            buf.drain(..body_start + len);
            return v;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return None;
        }
        let n = stdout.read_buf(buf).await.ok()?;
        if n == 0 {
            return None;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for RouterState {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(root) = params.root_uri {
            let path = root
                .to_file_path()
                .map(|p| p.join("compile_commands.json").exists())
                .unwrap_or(false);
            if path {
                self.client.log_message(
                    MessageType::INFO,
                    "[vaked-lsp] compile_commands.json found — clangd lane primed",
                ).await;
            }
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: Some(vec![".".to_string(), "->".to_string(), ":".to_string()]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "vaked-lsp-multiplexer".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        let mut subs = self.sub.lock().await;
        for (lane, sub) in subs.iter_mut() {
            if let Some(child) = sub.child.as_mut() {
                let _ = child.kill().await;
                let _ = self.client.log_message(MessageType::INFO, format!("[vaked-lsp] {lane} down")).await;
            }
        }
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.as_str().to_string();
        let params = serde_json::to_value(params).unwrap_or_default();
        let _ = self.forward(&uri, "textDocument/didOpen", &params).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.as_str().to_string();
        let params = serde_json::to_value(params).unwrap_or_default();
        let _ = self.forward(&uri, "textDocument/didChange", &params).await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.as_str().to_string();
        let params = serde_json::to_value(params).unwrap_or_default();
        Ok(self.forward(&uri, "textDocument/completion", &params).await
            .and_then(|v| serde_json::from_value(v.get("result").cloned().unwrap_or(v)).ok()))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri.as_str().to_string();
        let params = serde_json::to_value(params).unwrap_or_default();
        Ok(self.forward(&uri, "textDocument/hover", &params).await
            .and_then(|v| serde_json::from_value(v.get("result").cloned().unwrap_or(v)).ok()))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri.as_str().to_string();
        let params = serde_json::to_value(params).unwrap_or_default();
        Ok(self.forward(&uri, "textDocument/definition", &params).await
            .and_then(|v| serde_json::from_value(v.get("result").cloned().unwrap_or(v)).ok()))
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| RouterState {
        client,
        sub: Arc::new(Mutex::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::route_for;

    #[test]
    fn routes_every_lane() {
        assert_eq!(route_for("file:///game/Source/Wolf.cpp").unwrap().1, "ue-c++");
        assert_eq!(route_for("file:///engine/src/fauna.rs").unwrap().1, "rust");
        assert_eq!(route_for("file:///net/main.go").unwrap().1, "go");
        assert_eq!(route_for("file:///Interface/AddOns/meter/main.luau").unwrap().1, "luau");
        assert_eq!(route_for("file:///scaffold.sh").unwrap().1, "bash");
        assert!(route_for("file:///sandwich.png").is_none());
    }
    #[tokio::test]
    async fn document_notifications_do_not_wait_for_responses() {
        use super::*;
        use std::time::Duration;
        let script = r#"
import sys,json
def read():
    headers={}
    while True:
        line=sys.stdin.buffer.readline()
        if not line: raise EOFError()
        if line==b'\r\n': break
        k,v=line.decode().split(':',1);headers[k]=v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers['Content-Length'])))
for method in ['textDocument/didOpen','textDocument/didChange']:
    msg=read()
    assert msg['method']==method and 'id' not in msg, msg
msg=read()
assert msg['method']=='textDocument/hover' and 'id' in msg
body=json.dumps({'jsonrpc':'2.0','id':msg['id'],'result':{'contents':'synthetic hover'}}).encode()
sys.stdout.buffer.write(b'Content-Length: '+str(len(body)).encode()+b'\r\n\r\n'+body)
sys.stdout.buffer.flush()
"#;
        let child = Command::new("python3").args(["-u", "-c", script])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .kill_on_drop(true).spawn().unwrap();
        let mut subs = HashMap::new();
        // Already-initialized synthetic lane isolates notification forwarding.
        subs.insert("rust", SubServer { child: Some(child), next_id: 1, read_buf: Vec::new() });
        let (service, _socket) = LspService::new(|client| RouterState {
            client, sub: Arc::new(Mutex::new(subs)),
        });
        let state = service.inner();
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            for method in ["textDocument/didOpen", "textDocument/didChange"] {
                state.forward("file:///synthetic.rs", method, &serde_json::json!({})).await;
            }
            state.forward("file:///synthetic.rs", "textDocument/hover", &serde_json::json!({})).await
        }).await;
        let mut subs = state.sub.lock().await;
        if let Some(child) = subs.get_mut("rust").unwrap().child.as_mut() {
            let _ = child.kill().await;
        }
        let response = result.expect("notification forwarding blocked").expect("missing hover reply");
        assert_eq!(response["result"]["contents"], "synthetic hover");
    }

    #[tokio::test]
    async fn async_reader_rejects_oversized_frame_without_waiting() {
        use super::*;
        let mut child = Command::new("python3").args(["-u", "-c",
            "import sys,time;sys.stdout.write('Content-Length: 16777217\\r\\n\\r\\n');sys.stdout.flush();time.sleep(5)"])
            .stdout(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1),
            read_sub_frame(child.stdout.as_mut().unwrap(), &mut Vec::new())).await;
        let _ = child.kill().await;
        assert!(result.expect("reader waited for oversized body").is_none());
    }

}