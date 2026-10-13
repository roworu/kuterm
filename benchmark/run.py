#!/usr/bin/env python3
"""compare kuterm with alacritty and konsole: startup, pty throughput, memory and idle cpu.

every terminal gets the same font, a 100x30 grid and the same commands. see README.md.
"""

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
CACHE = HERE / ".cache"
RESULTS = HERE / "results"

FONT = "JetBrainsMonoNL Nerd Font Mono"
# kuterm sizes fonts in pixels, the others in points, 20px is 15pt at 96 dpi
FONT_PT = 15
COLUMNS, ROWS = 100, 30

VTEBENCH_REPO = "https://github.com/alacritty/vtebench"
# pinned so results stay comparable between runs
VTEBENCH_REV = "ead8003"


class Terminal:
    """how to launch one terminal so it runs `argv` in its only tab and closes after"""

    def __init__(self, name: str, binary: str):
        self.name = name
        self.binary = binary
        self.tmp = Path(tempfile.mkdtemp(prefix=f"bench-{name}-"))

    def command(self, argv: list[str]) -> tuple[list[str], dict]:
        raise NotImplementedError

    def env(self) -> dict:
        """fresh config and state folders, so saved window sizes and user settings stay out.
        data stays, fonts installed for the user live there"""
        base = Path(tempfile.mkdtemp(dir=self.tmp))
        return {"XDG_CONFIG_HOME": str(base / "config"), "XDG_STATE_HOME": str(base / "state")}

    def version(self) -> str:
        out = subprocess.run([self.binary, "--version"], capture_output=True, text=True)
        return (out.stdout or out.stderr).strip().splitlines()[0] if out.returncode == 0 else "?"


class Alacritty(Terminal):
    def __init__(self, binary: str):
        super().__init__("alacritty", binary)
        self.config = self.tmp / "alacritty.toml"
        self.config.write_text(
            f'[font]\nsize = {FONT_PT}\nnormal = {{ family = "{FONT}" }}\n'
            f"[window]\ndimensions = {{ columns = {COLUMNS}, lines = {ROWS} }}\n"
        )

    def command(self, argv):
        return [self.binary, "--config-file", str(self.config), "-e", *argv], self.env()


class Konsole(Terminal):
    def __init__(self, binary: str):
        super().__init__("konsole", binary)

    def command(self, argv):
        # its own process, so it can be timed and measured alone
        return [
            self.binary, "--separate", "--builtin-profile",
            "-p", f"Font={FONT},{FONT_PT}",
            "-p", f"TerminalColumns={COLUMNS}",
            "-p", f"TerminalRows={ROWS}",
            "-e", *argv,
        ], self.env()

    def version(self):
        out = subprocess.run([self.binary, "--version"], capture_output=True, text=True)
        return out.stdout.strip() or "?"


class Kuterm(Terminal):
    def __init__(self, binary: str):
        super().__init__("kuterm", binary)

    def command(self, argv):
        # kuterm has no -e, the command is the default profile of the throwaway config
        env = self.env()
        config = Path(env["XDG_CONFIG_HOME"])
        (config / "kuterm").mkdir(parents=True)
        settings = {
            "confirm_quit": "never",
            "terminal": {"font": {"family": FONT, "size": round(FONT_PT * 4 / 3)}},
            "window": {"columns": COLUMNS, "rows": ROWS},
            "profiles": [{
                "name": "bench",
                "default": True,
                "command": {"with_arguments": {"program": argv[0], "args": argv[1:]}},
            }],
        }
        (config / "kuterm" / "settings.jsonc").write_text(json.dumps(settings))
        return [self.binary], env

    def version(self):
        rev = subprocess.run(["git", "-C", str(REPO), "rev-parse", "--short", "HEAD"],
                             capture_output=True, text=True).stdout.strip()
        dirty = subprocess.run(["git", "-C", str(REPO), "status", "--porcelain", "--", "src"],
                               capture_output=True, text=True).stdout.strip()
        return f"kuterm {rev}{'+changes' if dirty else ''}"


def launch(term: Terminal, argv: list[str]) -> subprocess.Popen:
    cmd, env = term.command(argv)
    return subprocess.Popen(cmd, env={**os.environ, **env},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def run(term: Terminal, argv: list[str], timeout: float) -> float:
    """seconds from launch until the terminal exits"""
    start = time.perf_counter()
    proc = launch(term, argv)
    try:
        proc.wait(timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        raise RuntimeError(f"{term.name} did not exit within {timeout}s")
    return time.perf_counter() - start


def check_grid(term: Terminal) -> str:
    out = term.tmp / "grid"
    run(term, ["sh", "-c", f"echo $(tput cols)x$(tput lines) > {out}"], 30)
    return out.read_text().strip()


def startup(term: Terminal, runs: int) -> dict:
    """launch to the command running in the terminal, and launch to exit"""
    stamp = term.tmp / "stamp"
    to_shell, to_exit = [], []
    for i in range(runs + 1):
        stamp.unlink(missing_ok=True)
        start_ns = time.time_ns()
        took = run(term, ["sh", "-c", f"date +%s%N > {stamp}"], 30)
        if i == 0:
            continue  # warmup, fills the page cache and shader caches
        to_shell.append((int(stamp.read_text()) - start_ns) / 1e6)
        to_exit.append(took * 1000)
    return {"to_command_ms": summary(to_shell), "to_exit_ms": summary(to_exit)}


def proc_stats(pid: int) -> tuple[float, int]:
    """cpu seconds used and context switches of all threads of `pid`"""
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    ticks = int(fields[11]) + int(fields[12])
    switches = 0
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            for line in (task / "status").read_text().splitlines():
                if "ctxt_switches" in line:
                    switches += int(line.split()[1])
        except FileNotFoundError:
            pass
    return ticks / os.sysconf("SC_CLK_TCK"), switches


def pss_mb(pid: int) -> float:
    for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        if line.startswith("Pss:"):
            return int(line.split()[1]) / 1024
    return float("nan")


def idle(term: Terminal, settle: float, window: float) -> dict:
    """memory once started, then cpu and wakeups while the shell waits"""
    proc = launch(term, ["sleep", str(settle + window + 5)])
    try:
        time.sleep(settle)
        memory = pss_mb(proc.pid)
        cpu0, sw0 = proc_stats(proc.pid)
        time.sleep(window)
        cpu1, sw1 = proc_stats(proc.pid)
    finally:
        proc.kill()
        proc.wait()
    return {
        "pss_mb": round(memory, 1),
        "idle_cpu_percent": round((cpu1 - cpu0) / window * 100, 2),
        "idle_wakeups_per_s": round((sw1 - sw0) / window, 1),
    }


def build_vtebench() -> Path:
    src = CACHE / "vtebench"
    if not src.exists():
        CACHE.mkdir(exist_ok=True)
        subprocess.run(["git", "clone", "-q", VTEBENCH_REPO, str(src)], check=True)
    subprocess.run(["git", "-C", str(src), "checkout", "-q", VTEBENCH_REV], check=True)
    subprocess.run(["cargo", "build", "-q", "--release"], cwd=src, check=True)
    return src


def throughput(term: Terminal, vtebench: Path, max_secs: int) -> dict:
    """vtebench run inside the terminal, ms per sample of at least 1 MiB, lower is better"""
    dat = term.tmp / "vtebench.dat"
    dat.unlink(missing_ok=True)
    binary = vtebench / "target" / "release" / "vtebench"
    script = (f"cd {vtebench} && {binary} -s -b benchmarks "
              f"--max-secs {max_secs} --dat {dat}")
    run(term, ["sh", "-c", script], 60 * 30)
    lines = dat.read_text().splitlines()
    names = lines[0].split()
    columns = {name: [] for name in names}
    for line in lines[1:]:
        for name, value in zip(names, line.split()):
            if value != "_":
                columns[name].append(float(value))
    return {name: summary(samples) for name, samples in columns.items()}


def summary(samples: list[float]) -> dict:
    return {
        "median": round(statistics.median(samples), 2),
        "mean": round(statistics.fmean(samples), 2),
        "stdev": round(statistics.stdev(samples), 2) if len(samples) > 1 else 0.0,
        "min": round(min(samples), 2),
        "n": len(samples),
    }


def machine() -> dict:
    def first(cmd: str) -> str:
        out = subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout
        return out.strip().splitlines()[0] if out.strip() else "?"

    return {
        "cpu": first("grep -m1 'model name' /proc/cpuinfo | cut -d: -f2"),
        "gpu": first("lspci | grep -i ' vga ' | cut -d: -f3"),
        "kernel": platform.release(),
        "session": os.environ.get("XDG_SESSION_TYPE", "?"),
        "desktop": os.environ.get("XDG_CURRENT_DESKTOP", "?"),
    }


def report(results: dict) -> str:
    names = list(results["terminals"])
    head = "| | " + " | ".join(names) + " |\n|---|" + "---|" * len(names) + "\n"

    def row(label: str, get) -> str:
        return f"| {label} | " + " | ".join(get(results["terminals"][n]) for n in names) + " |\n"

    out = [f"# terminal benchmark {results['date']}\n\n"]
    out.append("| machine | |\n|---|---|\n")
    out += [f"| {k} | {v} |\n" for k, v in results["machine"].items()]
    out.append("\n" + head)
    out.append(row("version", lambda t: t["version"]))
    out.append(row("grid", lambda t: t["grid"]))
    if "startup" in results["tests"]:
        out.append(row("launch to command, median ms",
                       lambda t: f"{t['startup']['to_command_ms']['median']:.0f}"))
        out.append(row("launch to exit, median ms",
                       lambda t: f"{t['startup']['to_exit_ms']['median']:.0f}"))
    if "idle" in results["tests"]:
        out.append(row("memory, PSS MiB", lambda t: f"{t['idle']['pss_mb']:.0f}"))
        out.append(row("idle cpu %", lambda t: f"{t['idle']['idle_cpu_percent']:.2f}"))
        out.append(row("idle wakeups/s", lambda t: f"{t['idle']['idle_wakeups_per_s']:.0f}"))
    if "throughput" in results["tests"]:
        out.append("\nvtebench, median ms per sample (lower is better)\n\n" + head)
        benches = results["terminals"][names[0]]["throughput"]
        for bench in benches:
            out.append(row(bench, lambda t, b=bench: f"{t['throughput'][b]['median']:.2f}"))
    return "".join(out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kuterm", default=str(REPO / "target/podman/release/kuterm"))
    parser.add_argument("--alacritty", default=shutil.which("alacritty"))
    parser.add_argument("--konsole", default=shutil.which("konsole"))
    parser.add_argument("--tests", default="startup,idle,throughput")
    parser.add_argument("--startup-runs", type=int, default=20)
    parser.add_argument("--vtebench-secs", type=int, default=10)
    args = parser.parse_args()

    terms = []
    for cls, binary in [(Kuterm, args.kuterm), (Alacritty, args.alacritty),
                        (Konsole, args.konsole)]:
        if binary and Path(binary).exists():
            terms.append(cls(binary))
        else:
            print(f"skipping {cls.__name__.lower()}, not found", file=sys.stderr)
    tests = args.tests.split(",")
    vtebench = build_vtebench() if "throughput" in tests else None

    results = {"date": time.strftime("%Y-%m-%d %H:%M"), "machine": machine(),
               "tests": tests, "terminals": {}}
    for term in terms:
        print(f"== {term.name}", file=sys.stderr)
        entry = {"version": term.version(), "grid": check_grid(term)}
        if "startup" in tests:
            entry["startup"] = startup(term, args.startup_runs)
        if "idle" in tests:
            entry["idle"] = idle(term, settle=4, window=10)
        if "throughput" in tests:
            entry["throughput"] = throughput(term, vtebench, args.vtebench_secs)
        results["terminals"][term.name] = entry
        print(json.dumps(entry), file=sys.stderr)

    RESULTS.mkdir(exist_ok=True)
    stem = time.strftime("%Y-%m-%d-%H%M")
    (RESULTS / f"{stem}.json").write_text(json.dumps(results, indent=2))
    text = report(results)
    (RESULTS / f"{stem}.md").write_text(text)
    print(text)


if __name__ == "__main__":
    main()
