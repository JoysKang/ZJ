#!/usr/bin/env python3
"""Measure ZJ against the resource budget in CLAUDE.md, on macOS.

Runs the dist binary on temporary folders (settings, session, recovery and agent history in a
temporary directory too) and reports:

  1. cold start to first frame (median of --runs launches);
  2. idle physical footprint and CPU, one window;
  3. physical footprint with 3 windows and 20 open files;
  4. keystroke-to-frame latency p50 / p99 (System Events types the keys: the terminal needs
     Privacy & Security → Automation (System Events) and → Accessibility; --skip-latency skips).

--breakdown also measures 3 windows without files and 1 window with the 20 files, and prints
the `footprint` categories of the 3-window, 20-file run, to tell windows from documents.

Footprint counts the whole process tree (git children included), as CLAUDE.md asks.

    cargo build --profile dist --locked
    python3 tools/measure_budget.py [--output report.json]
"""
import argparse
import ctypes
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# (target, limit) from CLAUDE.md.
BUDGET = {
    "cold_start_ms": (250, 400),
    "idle_footprint_mb": (70, 100),
    "three_windows_20_docs_mb": (220, 300),
    "idle_cpu_p95_percent": (0.0, 0.1),
    "key_latency_p99_ms": (8.3, 16.7),
}


class RUsage(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(name, ctypes.c_uint64) for name in (
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins", "wired_size",
        "resident_size", "phys_footprint", "proc_start_abstime", "proc_exit_abstime",
        "child_user_time", "child_system_time", "child_pkg_idle_wkups", "child_interrupt_wkups",
        "child_pageins", "child_elapsed_abstime", "diskio_bytesread", "diskio_byteswritten")]


LIBPROC = None


def usage(pid):
    global LIBPROC
    if LIBPROC is None:
        LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        LIBPROC.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    result = RUsage()
    if LIBPROC.proc_pid_rusage(pid, 2, ctypes.byref(result)):
        return None
    return result


def tree(pid):
    out = subprocess.run(["ps", "-axo", "pid=,ppid="], capture_output=True, text=True, check=True)
    parents = {int(p): int(pp) for p, pp in (line.split() for line in out.stdout.splitlines())}
    members = {pid}
    while True:
        children = {p for p, pp in parents.items() if pp in members}
        if children <= members:
            return members
        members |= children


def footprint_mb(pid):
    total = 0
    for member in tree(pid):
        u = usage(member)
        if u:
            total += u.phys_footprint
    return total / 1_000_000


def cpu_seconds(pid):
    # proc_pid_rusage times are in Mach absolute units; on Apple Silicon timebase is 125/3.
    info = mach_timebase()
    total = 0
    for member in tree(pid):
        u = usage(member)
        if u:
            total += u.user_time + u.system_time
    return total * info / 1e9


def mach_timebase():
    class Timebase(ctypes.Structure):
        _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
    libc = ctypes.CDLL("/usr/lib/libSystem.dylib")
    tb = Timebase()
    libc.mach_timebase_info(ctypes.byref(tb))
    return tb.numer / tb.denom


class App:
    """The editor with its stderr read on a thread, events collected with timestamps."""

    def __init__(self, binary, args, env):
        self.started = time.monotonic()
        self.process = subprocess.Popen([str(binary), *map(str, args)], stderr=subprocess.PIPE,
                                        stdout=subprocess.DEVNULL, env=env, text=True)
        self.lines = []
        self.lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.process.stderr:
            with self.lock:
                self.lines.append((time.monotonic(), line.strip()))

    def wait_for(self, prefix, count=1, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                hits = [(t, l) for t, l in self.lines if l.startswith(prefix)]
            if len(hits) >= count:
                return hits
            if self.process.poll() is not None:
                raise RuntimeError(f"ZJ exited early (code {self.process.returncode})")
            time.sleep(0.01)
        raise TimeoutError(f"no {count}× '{prefix}' within {timeout} s")

    def events(self, prefix):
        with self.lock:
            return [l for _, l in self.lines if l.startswith(prefix)]

    def stop(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()


def fixture(base):
    """Three folders with 20 source files between them, copied from this repository."""
    sources = sorted((ROOT / "crates/app/src").glob("*.rs"))[:20]
    folders = [base / f"ws{i}" for i in range(3)]
    files = []
    for index, source in enumerate(sources):
        folder = folders[index % 3]
        folder.mkdir(parents=True, exist_ok=True)
        target = folder / source.name
        shutil.copy(source, target)
        files.append(target)
    return folders, files


def environment(base):
    env = dict(os.environ)
    env["ZJ_SETTINGS"] = str(base / "data/settings.json")
    env["ZJ_SESSION"] = str(base / "data/session.json")
    env["ZJ_AGENT_DB"] = str(base / "data/agent.db")
    return env


def value_of(line, key):
    for part in line.split():
        if part.startswith(key + "="):
            return float(part.split("=", 1)[1])
    raise ValueError(line)


def measure_cold_start(binary, folder, env, runs):
    wall, reported = [], []
    for _ in range(runs):
        app = App(binary, [folder], env)
        try:
            (t, line), = app.wait_for("event=first_frame", timeout=15)
            wall.append((t - app.started) * 1000)
            reported.append(value_of(line, "ms"))
        finally:
            app.stop()
        time.sleep(1)
    return statistics.median(wall), statistics.median(reported), max(wall)


def measure_idle(binary, folder, env, seconds, file=None, show_categories=False):
    """One window idle; with `file` a document is shown, otherwise the welcome page (whose
    logo cursor blinks while the window is in front)."""
    # The frame log tells whether idle CPU comes from frames (a wake-up) or from other work.
    app = App(binary, [folder, *([file] if file else [])], dict(env, ZJ_FRAME_LOG="1"))
    try:
        app.wait_for("event=first_frame", timeout=15)
        app.wait_for("event=refresh_finished", timeout=30)
        if file:
            app.wait_for("event=document_opened", timeout=30)
        time.sleep(10)
        frames_before = len(app.events("event=frame"))
        wakes_before = len(app.events("event=display_link state=woken"))
        footprints = []
        interval = 0.2
        cpu = []
        previous = cpu_seconds(app.process.pid)
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            time.sleep(interval)
            now = cpu_seconds(app.process.pid)
            cpu.append((now - previous) / interval * 100)
            previous = now
            footprints.append(footprint_mb(app.process.pid))
        cpu.sort()
        p95 = cpu[int(len(cpu) * 0.95) - 1] if cpu else 0.0
        if show_categories:
            frames = len(app.events("event=frame")) - frames_before
            wakes = len(app.events("event=display_link state=woken")) - wakes_before
            print(f"    测量期间出帧 {frames} 次，display link 被唤醒 {wakes} 次")
            for dirty, name in categories(app.process.pid):
                print(f"    {dirty:8.1f} MB  {name}")
        return statistics.median(footprints), p95
    finally:
        app.stop()


def categories(pid, top=8):
    """The largest `footprint` categories of the app process (dirty bytes)."""
    out = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split()
        # "  58 MB   0 B   0 B   3    IOSurface"
        if len(parts) >= 7 and parts[1] in ("B", "KB", "MB", "GB") and parts[-1] != "Category":
            scale = {"B": 1e-6, "KB": 1e-3, "MB": 1, "GB": 1e3}[parts[1]]
            try:
                dirty = float(parts[0]) * scale
            except ValueError:
                continue
            name = " ".join(parts[7:]) if len(parts) > 7 else parts[-1]
            if name != "TOTAL":
                rows.append((dirty, name))
    return sorted(rows, reverse=True)[:top]


def measure_many(binary, folders, files, env, show_categories=False):
    app = App(binary, [*folders, *files], env)
    try:
        if files:
            app.wait_for("event=document_opened", count=len(files), timeout=60)
        else:
            app.wait_for("event=refresh_finished", count=len(folders), timeout=60)
        time.sleep(10)
        value = statistics.median(footprint_mb(app.process.pid) for _ in range(10))
        if show_categories:
            for dirty, name in categories(app.process.pid):
                print(f"    {dirty:8.1f} MB  {name}")
            children = len(tree(app.process.pid)) - 1
            print(f"    子进程 {children} 个")
        return value
    finally:
        app.stop()


def measure_per_window(binary, base, env):
    """What one more window costs: footprint categories with 1 and with 3 empty folders open."""
    folders = []
    for index in range(3):
        folder = base / f"empty{index}"
        folder.mkdir(exist_ok=True)
        (folder / "README.md").write_text("empty\n")
        folders.append(folder)
    measured = []
    for count in (1, 3):
        app = App(binary, folders[:count], env)
        try:
            app.wait_for("event=refresh_finished", count=count, timeout=60)
            time.sleep(10)
            total = statistics.median(footprint_mb(app.process.pid) for _ in range(10))
            measured.append((total, dict((name, dirty) for dirty, name in
                                         categories(app.process.pid, top=None))))
        finally:
            app.stop()
    (one, one_rows), (three, three_rows) = measured
    print(f"每多一个窗口：{(three - one) / 2:.1f} MB（1 个窗口 {one:.1f} MB，3 个窗口 {three:.1f} MB）")
    deltas = sorted(((three_rows.get(n, 0) - one_rows.get(n, 0)) / 2, n)
                    for n in set(one_rows) | set(three_rows))
    for delta, name in reversed(deltas[-12:]):
        if delta > 0.05:
            print(f"    {delta:+8.1f} MB  {name}")
    return (three - one) / 2


def measure_latency(binary, folder, file, env, keys):
    env = dict(env, ZJ_LATENCY_LOG="1")
    app = App(binary, [folder, file], env)
    try:
        app.wait_for("event=document_opened", timeout=30)
        time.sleep(2)
        pid = app.process.pid
        script = f'''
tell application "System Events"
    set frontmost of (first process whose unix id is {pid}) to true
    delay 0.5
    repeat {keys} times
        keystroke "a"
        delay 0.03
    end repeat
end tell'''
        result = subprocess.run(["osascript", "-e", script], capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError("System Events 没能打字：请在 系统设置 → 隐私与安全性 → 自动化 里允许"
                               "终端控制 System Events，并在 辅助功能 里勾选终端，然后重开终端再试"
                               f"（{result.stderr.strip()}）")
        time.sleep(1)
        samples = sorted(value_of(l, "us") / 1000 for l in app.events("event=key_latency"))
        if len(samples) < keys // 2:
            raise RuntimeError(f"only {len(samples)} of {keys} keystrokes were logged")
        p99 = samples[min(len(samples) - 1, int(len(samples) * 0.99))]
        return statistics.median(samples), p99, len(samples)
    finally:
        app.stop()


def verdict(name, value):
    target, limit = BUDGET[name]
    if value <= target:
        return "达标"
    return "超目标" if value <= limit else "超上限"


def main():
    if sys.platform != "darwin":
        sys.exit("measure_budget.py measures macOS physical footprint; run it on a Mac")
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/dist/workspace-editor")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--idle-seconds", type=float, default=30)
    parser.add_argument("--keys", type=int, default=200)
    parser.add_argument("--skip-latency", action="store_true")
    parser.add_argument("--breakdown", action="store_true",
                        help="also 3 windows without files, 1 window with the files, categories")
    parser.add_argument("--per-window", action="store_true",
                        help="also what one more window costs, by footprint category")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not args.binary.is_file():
        sys.exit(f"{args.binary} is missing: cargo build --profile dist --locked")

    report = {"binary_bytes": args.binary.stat().st_size}
    with tempfile.TemporaryDirectory(prefix="zj-budget-") as temporary:
        base = Path(temporary)
        folders, files = fixture(base)
        env = environment(base)
        wall, reported, worst = measure_cold_start(args.binary, folders[0], env, args.runs)
        report["cold_start_ms"] = wall
        report["cold_start_reported_ms"] = reported
        report["cold_start_worst_ms"] = worst
        print(f"冷启动到首帧：中位 {wall:.0f} ms（应用自报 {reported:.0f} ms，最慢 {worst:.0f} ms）")
        # The budget's idle CPU is without the blinking logo cursor: a document is shown.
        idle, cpu = measure_idle(args.binary, folders[0], env, args.idle_seconds, files[0],
                                 show_categories=args.breakdown)
        report["idle_footprint_mb"] = idle
        report["idle_cpu_p95_percent"] = cpu
        print(f"空闲（1 个窗口，打开一个文件）：{idle:.1f} MB；CPU p95：{cpu:.3f}%")
        welcome, blink_cpu = measure_idle(args.binary, folders[0], env, args.idle_seconds,
                                          show_categories=args.breakdown)
        report["idle_welcome_footprint_mb"] = welcome
        report["idle_welcome_cpu_p95_percent"] = blink_cpu
        print(f"空闲（1 个窗口，欢迎页）：{welcome:.1f} MB；CPU p95：{blink_cpu:.3f}%")
        many = measure_many(args.binary, folders, files, env, show_categories=args.breakdown)
        report["three_windows_20_docs_mb"] = many
        print(f"3 个窗口 + {len(files)} 个文档：{many:.1f} MB")
        if args.breakdown:
            windows_only = measure_many(args.binary, folders, [], env, show_categories=True)
            report["three_windows_no_docs_mb"] = windows_only
            print(f"3 个窗口、不开文件：{windows_only:.1f} MB")
            # One folder that holds all the files (their common parent).
            docs_only = measure_many(args.binary, [folders[0].parent], files, env,
                                     show_categories=True)
            report["one_window_20_docs_mb"] = docs_only
            print(f"1 个窗口 + {len(files)} 个文档：{docs_only:.1f} MB")
        if args.per_window:
            report["per_window_mb"] = measure_per_window(args.binary, base, env)
        if not args.skip_latency:
            try:
                p50, p99, count = measure_latency(args.binary, folders[0], files[0], env, args.keys)
                report["key_latency_p50_ms"] = p50
                report["key_latency_p99_ms"] = p99
                print(f"按键到画面（{count} 次）：p50 {p50:.2f} ms，p99 {p99:.2f} ms")
            except RuntimeError as error:
                print(f"按键延迟未测：{error}")

    print()
    print(f"{'指标':<24}{'实测':>10}{'目标':>10}{'上限':>10}  结论")
    for name, (target, limit) in BUDGET.items():
        if name in report:
            value = report[name]
            print(f"{name:<24}{value:>10.2f}{target:>10}{limit:>10}  {verdict(name, value)}")
    print(f"二进制体积：{report['binary_bytes'] / 1_000_000:.2f} MB（目标 30 MB）")
    if args.output:
        args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
