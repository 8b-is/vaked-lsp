// vaked-lsp — the all-in-one LSP gateway, and its sidecar: the umbrella MCP.
//
// One crate, two doors:
//   vaked-lsp  — the LSP multiplexer (editor-visible door): one endpoint in
//                front of clangd / rust-analyzer / gopls / luau-lsp / bash-ls
//   vaked-mcp  — the umbrella MCP sidecar (agent-visible door): one MCP
//                endpoint in front of Unreal, Unity, FAB, the UE toolchain,
//                and the local offline generation lanes (osarous).
//
// Shared substrate: the framing (Content-Length JSON-RPC), the router table
// (one lane per language/engine), the doctrine (one door, many lanes — the
// lane stays dark when its engine is not running, never a hard failure).

pub mod frame;