# mlx-sidecar — fast top-SWE-score coders, local

The MLX coder sidecar: supervises `mlx_lm.server` instances
(OpenAI-compatible, Metal) for the constellation's coding lanes. The
catalog is curated for **speed + SWE-bench**, uses **abliterated** (no
refusal) weights, and prefers **pre-converted MLX** versions for Apple
Silicon. Every launch first checks the host's free memory and picks the
model that fits.

## the catalog

| Lane | Model | Port | min RAM | Notes |
|---|---|---|---|---|
| `qwen3-coder-next-oblit-mlx` | Huihui-Qwen3-Coder-Next-abliterated-mlx-4Bit | 1340 | 20 GB | abliterated + MLX 4-bit · top SWE-bench |
| `qwen3-coder-30b-a3b-oblit` | Huihui-Qwen3-Coder-30B-A3B-Instruct-abliterated | 1341 | 20 GB | ~70 SWE-bench Verified · 3B active MoE |
| `qwen25-coder-7b-oblit` | OBLITERATUS/Qwen2.5-Coder-7B-Instruct-OBLITERATED | 1342 | 6 GB | ~40 SWE-bench · the fastest lane |
| `qwen3-42b-oblit-mlx` | Qwen3-42B-A3B MASTER-CODER qx4-mlx | 1343 | 24 GB | 42B A3B abliterated · the big lane |

## memory-aware launch

```bash
uv run python sidecar.py memory                  # host memory + best-fit lane
uv run python sidecar.py models                  # catalog with fit vs. free memory
uv run python sidecar.py start                   # auto-picks the best fit by free memory
uv run python sidecar.py start qwen25-coder-7b-oblit   # or name a lane
uv run python sidecar.py start --force qwen3-42b-oblit-mlx  # override the fit check
uv run python sidecar.py status                  # which lanes are up
uv run python sidecar.py stop qwen25-coder-7b-oblit
```

Host memory is read from `vm_stat` + `sysctl` on macOS and `/proc/meminfo`
on Linux; a lane is only launched when its estimated footprint fits the
available memory (with an 85% safety margin).

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
