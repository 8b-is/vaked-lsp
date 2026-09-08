#!/usr/bin/env python3
"""vaked mlx-sidecar — fast top-SWE-score coders, running locally on MLX.

Supervises mlx_lm.server instances (OpenAI-compatible, Metal) for the
constellation's coding lanes. The catalog is curated for speed + SWE-bench:
MoE coders with few active parameters run fastest on Apple Silicon.

Run with uv:  uv run python sidecar.py <command> [args]
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PORT = int(os.environ.get("MLX_SIDECAR_PORT", "1340"))

# the catalog: fast + top-SWE-score coders, mlx-community 4-bit quants.
# swe = approximate SWE-bench Verified (agentic) for the base model.
CATALOG = {
    "qwen3-coder-30b-a3b": {
        "model": "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit",
        "port": 1340, "swe": "~70 SWE-bench Verified · 3B active (MoE) · the fast top",
    },
    "qwen25-coder-14b": {
        "model": "mlx-community/Qwen2.5-Coder-14B-Instruct-4bit",
        "port": 1341, "swe": "~50 SWE-bench Verified · dense 14B · solid + fast",
    },
    "qwen25-coder-7b": {
        "model": "mlx-community/Qwen2.5-Coder-7B-Instruct-4bit",
        "port": 1342, "swe": "~40 SWE-bench Verified · dense 7B · the fastest lane",
    },
    "deepseek-coder-v2-lite": {
        "model": "mlx-community/DeepSeek-Coder-V2-Lite-Instruct-4bit",
        "port": 1343, "swe": "MoE 16B (2.4B active) · the classic coder MoE",
    },
}


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


def cmd_models(_args):
    for name, c in CATALOG.items():
        state = "up" if _running(name) else "down"
        print(f"{name:24} port {c['port']}  {state:4}  {c['swe']}")


def cmd_status(_args):
    for name, c in CATALOG.items():
        up = _running(name)
        healthy = _health(c["port"]) if up else False
        print(f"{name:24} port {c['port']}  {'up' if up else 'down':4}  {'healthy' if healthy else ''}")


def cmd_start(args):
    name = args.model
    if name not in CATALOG:
        sys.exit(f"unknown model {name!r} — catalog: {', '.join(CATALOG)}")
    if _running(name):
        print(f"{name} already running on port {CATALOG[name]['port']}")
        return
    c = CATALOG[name]
    cmd = [
        sys.executable, "-m", "mlx_lm.server",
        "--model", c["model"],
        "--port", str(c["port"]),
    ]
    if args.quantized:
        cmd.append("--quantized")
    log = open(os.path.join(HERE, f".{name}.log"), "a")
    proc = subprocess.Popen(cmd, stdout=log, stderr=log, start_new_session=True)
    with open(_pid_file(name), "w") as f:
        f.write(str(proc.pid))
    print(f"starting {name} (pid {proc.pid}) on port {c['port']} — first load pays the cold start")
    for _ in range(60):
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
    if _health(c["port"]):
        print(f"{name} healthy at http://127.0.0.1:{c['port']}/v1")
    else:
        print(f"{name} down on port {c['port']}")


def main():
    p = argparse.ArgumentParser(prog="mlx-sidecar", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("models", help="list the fast top-SWE-score catalog").set_defaults(fn=cmd_models)
    sub.add_parser("status", help="which lanes are up and healthy").set_defaults(fn=cmd_status)

    ps = sub.add_parser("start", help="start a coder lane")
    ps.add_argument("model")
    ps.add_argument("--quantized", action="store_true", help="force --quantized on load")
    ps.set_defaults(fn=cmd_start)

    pp = sub.add_parser("stop", help="stop a coder lane")
    pp.add_argument("model")
    pp.set_defaults(fn=cmd_stop)

    ph = sub.add_parser("health", help="probe one lane's OpenAI endpoint")
    ph.add_argument("model")
    ph.set_defaults(fn=cmd_health)

    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
