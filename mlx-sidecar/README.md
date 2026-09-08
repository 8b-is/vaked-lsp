# mlx-sidecar — fast top-SWE-score coders, local

The MLX coder sidecar: supervises `mlx_lm.server` instances
(OpenAI-compatible, Metal) for the constellation's coding lanes. The
catalog is curated for **speed + SWE-bench** — MoE coders with few active
parameters run fastest on Apple Silicon.

## the catalog

| Lane | Model (mlx-community 4-bit) | Port | SWE-bench |
|---|---|---|---|
| `qwen3-coder-30b-a3b` | Qwen3-Coder-30B-A3B-Instruct | 1340 | ~70 Verified · 3B active (MoE) — the fast top |
| `qwen25-coder-14b` | Qwen2.5-Coder-14B-Instruct | 1341 | ~50 Verified · dense 14B |
| `qwen25-coder-7b` | Qwen2.5-Coder-7B-Instruct | 1342 | ~40 Verified · the fastest lane |
| `deepseek-coder-v2-lite` | DeepSeek-Coder-V2-Lite-Instruct | 1343 | MoE 16B (2.4B active) |

## usage

```bash
uv run python sidecar.py models                  # the catalog
uv run python sidecar.py status                  # which lanes are up
uv run python sidecar.py start qwen3-coder-30b-a3b   # first load pays the cold start
uv run python sidecar.py health qwen3-coder-30b-a3b  # probe the OpenAI endpoint
uv run python sidecar.py stop qwen3-coder-30b-a3b
```

Each lane is an OpenAI-compatible endpoint:
`http://127.0.0.1:<port>/v1` — point any OpenAI client (aider, entheai,
opencode, the 8b.is gateway) at it.

## MCP wiring

`vaked-mcp` (the umbrella sidecar) exposes four tools that dispatch to this
lane: `mlx_status`, `mlx_models`, `mlx_start`, `mlx_stop`. The lane stays
dark when the uv project is missing.

## notes

- Models download on first `start` (HuggingFace, mlx-community 4-bit).
- The sidecar writes `. <lane>.pid` / `. <lane>.log` next to `sidecar.py`.
- Metal required (Apple Silicon); `mlx_lm` is the serving engine.

— the constellation · 0 + 1 · fine touch from within · vaked.dev
