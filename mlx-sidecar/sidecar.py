#!/usr/bin/env python3
"""vaked mlx-sidecar — local Metal lanes: coders, vision, and a diffuser.

Supervises mlx_lm.server / mlx_vlm.server instances (OpenAI-compatible,
Metal) for the constellation's coding, vision, and image-generation
lanes. Every launch first checks the host's utilized / free memory
(macOS vm_stat + sysctl, Linux /proc/meminfo) and picks the model that
fits — the fastest lane the machine can actually hold.

Run with uv:  uv run python sidecar.py <command> [args]
"""

import argparse
import base64
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

# the catalog: three lane kinds —
#   coder     fast top-SWE-score coders, ABLITERATED (no refusal)
#   vision    small vision-language model for art QA / render review
#   diffuser  FLUX.1-schnell concept-art lane (one-shot, not a server)
# swe = approximate SWE-bench Verified (agentic). min_ram = estimated
# resident footprint (weights + KV cache + runtime headroom) in GB.
CATALOG = {
    "qwen3-coder-next-oblit-mlx": {
        "kind": "coder",
        "model": "Eldadalbajob/Huihui-Qwen3-Coder-Next-abliterated-mlx-4Bit",
        "port": 1340, "min_ram": 20.0,
        "swe": "Qwen3-Coder-Next abliterated · MLX 4-bit · top SWE-bench · the fast top",
    },
    "qwen3-coder-30b-a3b-oblit": {
        "kind": "coder",
        "model": "huihui-ai/Huihui-Qwen3-Coder-30B-A3B-Instruct-abliterated",
        "port": 1341, "min_ram": 20.0,
        "swe": "~70 SWE-bench Verified · 3B active (MoE) · abliterated",
    },
    "qwen25-coder-7b-oblit": {
        "kind": "coder",
        "model": "OBLITERATUS/Qwen2.5-Coder-7B-Instruct-OBLITERATED",
        "port": 1342, "min_ram": 6.0,
        "swe": "~40 SWE-bench Verified · abliterated · the fastest lane",
    },
    "qwen3-42b-oblit-mlx": {
        "kind": "coder",
        "model": "nightmedia/Qwen3-42B-A3B-2507-Thinking-Abliterated-uncensored-TOTAL-RECALL-v2-Medium-MASTER-CODER-qx4-mlx",
        "port": 1343, "min_ram": 24.0,
        "swe": "42B A3B abliterated · pre-converted MLX qx4 · the big lane",
    },
    "qwen25-vl-3b-mlx": {
        "kind": "vision",
        "model": "mlx-community/Qwen2.5-VL-3B-Instruct-4bit",
        "port": 1344, "min_ram": 4.0,
        "swe": "Qwen2.5-VL 3B 4-bit · render/art QA · the vision lane",
    },
    "flux2-klein-mlx": {
        "kind": "diffuser",
        "model": "flux2-klein-4b",
        "exe": "mflux-generate-flux2",
        "port": 0, "min_ram": 5.0,
        "swe": "FLUX.2-klein 4B quantize-4 · concept art · one-shot lane",
    },
}

# order of preference when auto-selecting: highest SWE score first (coders only)
PREFERENCE = ["qwen3-coder-next-oblit-mlx", "qwen3-coder-30b-a3b-oblit", "qwen25-coder-7b-oblit", "qwen3-42b-oblit-mlx"]

# safety margin: keep this fraction of available RAM for the OS + KV growth
MARGIN = 0.85


# ── host memory ────────────────────────────────────────────────────────────
def _host_memory():
    """Return (total_gb, available_gb) for macOS and Linux."""
    if sys.platform == "darwin":
        try:
            total = int(subprocess.check_output(
                ["sysctl", "-n", "hw.memsize"]).strip())
            page = int(subprocess.check_output(
                ["sysctl", "-n", "vm.pagesize"]).strip())
            out = subprocess.check_output(["vm_stat"]).decode()
            vals = {}
            for line in out.splitlines():
                line = line.strip()
                if ":" not in line:
                    continue
                key, _, val = line.partition(":")
                key = key.strip().replace(" ", "_")
                val = val.strip().rstrip(".")
                if val:
                    try:
                        vals[key] = int(val)
                    except ValueError:
                        pass
            free = vals.get("Pages_free", 0)
            inactive = vals.get("Pages_inactive", 0)
            speculative = vals.get("Pages_speculative", 0)
            available = (free + inactive + speculative) * page
            return total / 2**30, available / 2**30
        except Exception:
            return 0.0, 0.0
    elif sys.platform.startswith("linux"):
        try:
            meminfo = {}
            for line in open("/proc/meminfo"):
                key, _, val = line.partition(":")
                meminfo[key.strip()] = int(val.strip().split()[0]) * 1024
            total = meminfo.get("MemTotal", 0)
            available = meminfo.get("MemAvailable", total)
            return total / 2**30, available / 2**30
        except Exception:
            return 0.0, 0.0
    return 0.0, 0.0


def _pid_file(name: str) -> str:
    return os.path.join(HERE, f".{name}.pid")


def _health(port: int) -> bool:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/v1/models", timeout=2):
            return True
    except Exception:
        return False


def _running(name: str) -> bool:
    pid_file = _pid_file(name)
    if not os.path.exists(pid_file):
        return False
    try:
        pid = int(open(pid_file).read().strip())
        os.kill(pid, 0)
        return True
    except Exception:
        return False


def _server_cmd(name, c):
    """Launch command for server-kind lanes (coder + vision)."""
    if c["kind"] == "vision":
        return [sys.executable, "-m", "mlx_vlm.server",
                "--model", c["model"], "--port", str(c["port"])]
    return [sys.executable, "-m", "mlx_lm.server",
            "--model", c["model"], "--port", str(c["port"])]


def best_fit(available_gb: float):
    """Highest-SWE coder lane whose min_ram fits available (with margin)."""
    usable = available_gb * MARGIN
    for name in PREFERENCE:
        if CATALOG[name]["min_ram"] <= usable:
            return name
    return None


def cmd_memory(_args):
    total, avail = _host_memory()
    used = total - avail
    print(f"host: {sys.platform}")
    print(f"total:     {total:7.1f} GB")
    print(f"available: {avail:7.1f} GB")
    print(f"utilized:  {used:7.1f} GB ({100 * used / total:.0f}%)" if total else "utilized: n/a")
    fit = best_fit(avail)
    print(f"best fit:  {fit or 'none — free memory first'}")


def cmd_models(_args):
    total, avail = _host_memory()
    fit = best_fit(avail)
    for name, c in CATALOG.items():
        state = "up" if _running(name) else "down"
        fits = "fits" if c["min_ram"] <= avail * MARGIN else "too big"
        mark = "  <-- best fit" if name == fit else ""
        print(f"{name:24} {c['kind']:8} port {c['port']:5} {state:4}  min {c['min_ram']:4.1f}GB  {fits:8}{mark}")
    print(f"host: {avail:.1f} GB available of {total:.1f} GB")


def cmd_status(_args):
    for name, c in CATALOG.items():
        up = _running(name)
        if c["kind"] == "diffuser":
            print(f"{name:24} {c['kind']:8}  {'one-shot' if shutil.which(c.get('exe', 'mflux-generate')) else 'missing'}")
            continue
        healthy = _health(c["port"]) if up else False
        print(f"{name:24} {c['kind']:8} port {c['port']}  {'up' if up else 'down':4}  {'healthy' if healthy else ''}")


def cmd_start(args):
    total, avail = _host_memory()
    if avail <= 0:
        print("could not read host memory — launching anyway")
    else:
        print(f"host memory: {avail:.1f} GB available of {total:.1f} GB")

    name = args.model or best_fit(avail)
    if name is None:
        sys.exit("no model fits the available memory — free memory first, or set --force")
    if name not in CATALOG:
        sys.exit(f"unknown model {name!r} — catalog: {', '.join(CATALOG)}")

    c = CATALOG[name]
    if c["kind"] == "diffuser":
        sys.exit("the diffuser lane is one-shot — use the `image` command, not `start`")
    if not args.force and avail > 0 and c["min_ram"] > avail * MARGIN:
        fit = best_fit(avail)
        print(f"{name} needs ~{c['min_ram']:.1f} GB but only {avail:.1f} GB is available")
        if fit:
            print(f"the best fit is {fit} (~{CATALOG[fit]['min_ram']:.1f} GB) — launch it with --model {fit}, or --force to try anyway")
        sys.exit(1)

    if _running(name):
        print(f"{name} already running on port {c['port']}")
        return

    cmd = _server_cmd(name, c)
    if args.quantized and c["kind"] != "vision":
        cmd.append("--quantized")
    log = open(os.path.join(HERE, f".{name}.log"), "a")
    proc = subprocess.Popen(cmd, stdout=log, stderr=log, start_new_session=True)
    with open(_pid_file(name), "w") as f:
        f.write(str(proc.pid))
    print(f"starting {name} (pid {proc.pid}) on port {c['port']} — first load pays the cold start")
    for _ in range(180):
        if _health(c["port"]):
            print(f"{name} healthy at http://127.0.0.1:{c['port']}/v1")
            return
        time.sleep(1)
    print(f"{name} not healthy yet — check .{name}.log")


def cmd_stop(args):
    name = args.model
    if name not in CATALOG:
        sys.exit(f"unknown model {name!r} — catalog: {', '.join(CATALOG)}")
    pid_file = _pid_file(name)
    if not os.path.exists(pid_file):
        print(f"{name} not running")
        return
    pid = int(open(pid_file).read().strip())
    try:
        os.killpg(os.getpgid(pid), signal.SIGTERM)
    except Exception:
        try:
            os.kill(pid, signal.SIGTERM)
        except Exception:
            pass
    os.remove(pid_file)
    print(f"stopped {name}")


def cmd_health(args):
    name = args.model
    c = CATALOG[name]
    if c["kind"] == "diffuser":
        print("diffuser lane is one-shot — `image` command")
        return
    if _health(c["port"]):
        print(f"{name} healthy at http://127.0.0.1:{c['port']}/v1")
    else:
        print(f"{name} down on port {c['port']}")


# ── vision lane ────────────────────────────────────────────────────────────
def _vision_http(port, model_id, image_path, prompt, max_tokens):
    ext = os.path.splitext(image_path)[1].lstrip(".").lower() or "png"
    mime = "image/jpeg" if ext in ("jpg", "jpeg") else f"image/{ext}"
    b64 = base64.b64encode(open(image_path, "rb").read()).decode()
    body = json.dumps({
        "model": model_id,  # the server loads per-request — send the real repo
        "max_tokens": max_tokens,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{b64}"}},
            ],
        }],
    }).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        data = json.load(r)
    return data["choices"][0]["message"]["content"]


def cmd_vision(args):
    if not os.path.exists(args.image):
        sys.exit(f"image not found: {args.image}")
    c = CATALOG["qwen25-vl-3b-mlx"]
    if _health(c["port"]):
        out = _vision_http(c["port"], c["model"], args.image, args.prompt, args.max_tokens)
    else:
        print("vision server down — one-shot mlx_vlm.generate (cold start pays the load)")
        cmd = [sys.executable, "-m", "mlx_vlm.generate",
               "--model", c["model"], "--image", args.image,
               "--prompt", args.prompt, "--max-tokens", str(args.max_tokens)]
        out = subprocess.check_output(cmd).decode().strip()
    print(out)


# ── diffuser lane ──────────────────────────────────────────────────────────
def cmd_image(args):
    lane = CATALOG["flux2-klein-mlx"]
    exe = shutil.which(lane["exe"])
    if not exe:
        sys.exit(f"{lane['exe']} not on PATH — run `uv add mflux` in this project first")
    outdir = os.path.abspath(args.out)
    os.makedirs(outdir, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = os.path.join(outdir, f"flux-{stamp}.png")
    model = lane["model"]
    cmd = [exe, "--model", model, "--prompt", args.prompt,
           "--steps", str(args.steps), "--width", str(args.width),
           "--height", str(args.height), "--quantize", str(args.quantize),
           "--output", out]
    if args.seed is not None:
        cmd += ["--seed", str(args.seed)]
    print(f"generating {model} {args.width}x{args.height} steps={args.steps} q={args.quantize}")
    subprocess.check_call(cmd)
    print(f"wrote {out}")


def main():
    p = argparse.ArgumentParser(prog="mlx-sidecar", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("memory", help="report host memory + the best-fit lane").set_defaults(fn=cmd_memory)
    sub.add_parser("models", help="list the catalog with fit vs. host memory").set_defaults(fn=cmd_models)
    sub.add_parser("status", help="which lanes are up and healthy").set_defaults(fn=cmd_status)

    ps = sub.add_parser("start", help="start a coder or vision lane (auto-picks the best coder when no model given)")
    ps.add_argument("model", nargs="?", help="lane name, or omit to auto-select by free memory")
    ps.add_argument("--quantized", action="store_true", help="force --quantized on load (coders)")
    ps.add_argument("--force", action="store_true", help="launch even if the lane exceeds available memory")
    ps.set_defaults(fn=cmd_start)

    pp = sub.add_parser("stop", help="stop a coder or vision lane")
    pp.add_argument("model")
    pp.set_defaults(fn=cmd_stop)

    ph = sub.add_parser("health", help="probe one lane's OpenAI endpoint")
    ph.add_argument("model")
    ph.set_defaults(fn=cmd_health)

    pv = sub.add_parser("vision", help="ask the vision lane about an image (render/art QA)")
    pv.add_argument("image", help="path to the image")
    pv.add_argument("prompt", help="question about the image")
    pv.add_argument("--max-tokens", type=int, default=256)
    pv.set_defaults(fn=cmd_vision)

    pi = sub.add_parser("image", help="generate concept art with FLUX.1-schnell")
    pi.add_argument("prompt", help="image prompt")
    pi.add_argument("--steps", type=int, default=4)
    pi.add_argument("--width", type=int, default=1024)
    pi.add_argument("--height", type=int, default=1024)
    pi.add_argument("--quantize", type=int, choices=[4, 8], default=4)
    pi.add_argument("--seed", type=int, default=None)
    pi.add_argument("--out", default="out", help="output directory (default: out)")
    pi.set_defaults(fn=cmd_image)

    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
