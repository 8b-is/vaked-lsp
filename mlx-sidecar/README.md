# mlx-sidecar — coders, vision, and a diffuser, local

The MLX sidecar: supervises `mlx_lm.server` / `mlx_vlm.server` instances
(OpenAI-compatible, Metal) for the constellation's lanes. The coder
catalog is curated for **speed + SWE-bench**, uses **abliterated** (no
refusal) weights, and prefers **pre-converted MLX** versions for Apple
Silicon. Every launch first checks the host's free memory and picks the
model that fits. Two new lanes serve the game: a **vision** lane for
render/art QA, and a **diffuser** lane for FLUX.1-schnell concept art.

## the catalog

| Lane | Model | Port | min RAM | Notes |
|---|---|---|---|---|
| `qwen3-coder-next-oblit-mlx` | Huihui-Qwen3-Coder-Next-abliterated-mlx-4Bit | 1340 | 20 GB | abliterated + MLX 4-bit · top SWE-bench |
| `qwen3-coder-30b-a3b-oblit` | Huihui-Qwen3-Coder-30B-A3B-Instruct-abliterated | 1341 | 20 GB | ~70 SWE-bench Verified · 3B active MoE |
| `qwen25-coder-7b-oblit` | OBLITERATUS/Qwen2.5-Coder-7B-Instruct-OBLITERATED | 1342 | 6 GB | ~40 SWE-bench · the fastest lane |
| `qwen3-42b-oblit-mlx` | Qwen3-42B-A3B MASTER-CODER qx4-mlx | 1343 | 24 GB | 42B A3B abliterated · the big lane |
| `qwen25-vl-3b-mlx` | Qwen2.5-VL-3B-Instruct-4bit | 1344 | 4 GB | vision · render/art QA on the export lane |
| `flux2-klein-mlx` | FLUX.2-klein 4B (mflux, quantize-4) | — | 5 GB | diffuser · one-shot concept art |

## memory-aware launch

```bash
uv run python sidecar.py memory                  # host memory + best-fit lane
uv run python sidecar.py models                  # catalog with fit vs. free memory
uv run python sidecar.py start                   # auto-picks the best coder by free memory
uv run python sidecar.py start qwen25-coder-7b-oblit   # or name a lane
uv run python sidecar.py start qwen25-vl-3b-mlx  # the vision server (port 1344)
uv run python sidecar.py status                  # which lanes are up
uv run python sidecar.py stop qwen25-coder-7b-oblit
```

Host memory is read from `vm_stat` + `sysctl` on macOS and `/proc/meminfo`
on Linux; a lane is only launched when its estimated footprint fits the
available memory (with an 85% safety margin).

Each server lane is an OpenAI-compatible endpoint:
`http://127.0.0.1:<port>/v1` — point any OpenAI client (aider, entheai,
opencode, the 8b.is gateway) at it. The vision server also speaks
chat-completions with image parts.

## the vision lane (render/art QA)

```bash
uv run python sidecar.py start qwen25-vl-3b-mlx                 # serve the eye
uv run python sidecar.py vision renders/mahakala-tent.png \
  "is this frame admissible? describe composition, palette, and one defect"
```

Uses the running server when healthy; falls back to a one-shot
`mlx_vlm.generate` (cold start) otherwise.

## the diffuser lane (concept art)

```bash
uv run python sidecar.py image "the pink tent of MAHĀKĀLA pitched on the
  sanctuary floor, oklch gold accents, matte painting" --steps 4 --seed 108
```

`mflux` runs FLUX.2-klein 4B quantize-4 one-shot; outputs land in `out/`
(`--out <dir>` to redirect). First run downloads the weights. FLUX.1-schnell
is gated on HuggingFace; klein is the Apache-2.0 lane.

## MCP wiring

`vaked-mcp` (the umbrella sidecar) exposes six tools that dispatch to
these lanes: `mlx_status`, `mlx_models`, `mlx_start`, `mlx_stop`,
`mlx_vision`, `mlx_image`. The lane stays dark when the uv project is
missing.

## notes

- Models download on first `start` (HuggingFace, mlx-community 4-bit).
- The sidecar writes `. <lane>.pid` / `. <lane>.log` next to `sidecar.py`.
- Metal required (Apple Silicon); `mlx_lm` / `mlx_vlm` serve, `mflux`
  generates.

— the constellation · 0 + 1 · fine touch from within · vaked.dev
