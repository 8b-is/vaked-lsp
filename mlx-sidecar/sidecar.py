#!/usr/bin/env python3
"""vaked mlx-sidecar — fast top-SWE-score coders, running locally on MLX.

Supervises mlx_lm.server instances (OpenAI-compatible, Metal) for the
constellation's coding lanes. Every launch first checks the host's
utilized / free memory (macOS vm_stat + sysctl, Linux /proc/meminfo) and
picks the model that fits — the fast top-SWE lane that the machine can
actually hold.

Run with uv:  uv run python sidecar.py <command> [args]
"""

import argparse
import os
import signal
import subprocess
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

# the catalog: fast + top-SWE-score coders, ABLITERATED (no refusal) and
# macOS-Silicon-optimized (pre-converted MLX where available).
# swe = approximate SWE-bench Verified (agentic). min_ram = estimated
# resident footprint (weights + KV cache + runtime headroom) in GB.
CATALOG = {
    "qwen3-coder-next-oblit-mlx": {
        "model": "Eldadalbajob/Huihui-Qwen3-Coder-Next-abliterated-mlx-4Bit",
        "port": 1340, "min_ram": 20.0,
        "swe": "Qwen3-Coder-Next abliterated · MLX 4-bit · top SWE-bench · the fast top",
    },
    "qwen3-coder-30b-a3b-oblit": {
        "model": "huihui-ai/Huihui-Qwen3-Coder-30B-A3B-Instruct-abliterated",
        "port": 1341, "min_ram": 20.0,
        "swe": "~70 SWE-bench Verified · 3B active (MoE) · abliterated",
    },
    "qwen25-coder-7b-oblit": {
        "model": "OBLITERATUS/Qwen2.5-Coder-7B-Instruct-OBLITERATED",
        "port": 1342, "min_ram": 6.0,
        "swe": "~40 SWE-bench Verified · abliterated · the fastest lane",
    },
    "qwen3-42b-oblit-mlx": {
        "model": "nightmedia/Qwen3-42B-A3B-2507-Thinking-Abliterated-uncensored-TOTAL-RECALL-v2-Medium-MASTER-CODER-qx4-mlx",
        "port": 1343, "min_ram": 24.0,
        "swe": "42B A3B abliterated · pre-converted MLX qx4 · the big lane",
    },
}

# order of preference when auto-selecting: highest SWE score first
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


def best_fit(available_gb: float):
    """Highest-SWE lane whose min_ram fits available (with margin)."""
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
        print(f"{name:24} port {c['port']}  {state:4}  min {c['min_ram']:4.1f}GB  {fits:8}{mark}")
    print(f"host: {avail:.1f} GB available of {total:.1f} GB")


def cmd_status(_args):
    for name, c in CATALOG.items():
        up = _running(name)
        healthy = _health(c["port"]) if up else False
        print(f"{name:24} port {c['port']}  {'up' if up else 'down':4}  {'healthy' if healthy else ''}")


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
    if not args.force and avail > 0 and c["min_ram"] > avail * MARGIN:
        fit = best_fit(avail)
        print(f"{name} needs ~{c['min_ram']:.1f} GB but only {avail:.1f} GB is available")
        if fit:
            print(f"the best fit is {fit} (~{CATALOG[fit]['min_ram']:.1f} GB) — launch it with --model {fit}, or --force to try anyway")
        sys.exit(1)

    if _running(name):
        print(f"{name} already running on port {c['port']}")
        return

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

    sub.add_parser("memory", help="report host memory + the best-fit lane").set_defaults(fn=cmd_memory)
    sub.add_parser("models", help="list the catalog with fit vs. host memory").set_defaults(fn=cmd_models)
    sub.add_parser("status", help="which lanes are up and healthy").set_defaults(fn=cmd_status)

    ps = sub.add_parser("start", help="start a coder lane (auto-picks the best fit when no model given)")
    ps.add_argument("model", nargs="?", help="lane name, or omit to auto-select by free memory")
    ps.add_argument("--quantized", action="store_true", help="force --quantized on load")
    ps.add_argument("--force", action="store_true", help="launch even if the lane exceeds available memory")
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
