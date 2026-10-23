#!/usr/bin/env python3
"""Measure ZJ against the resource budget in CLAUDE.md, on macOS.

Runs the dist binary on temporary folders (settings, session, recovery and agent history in a
temporary directory too) and reports:

  1. cold start to first frame (median of --runs launches);
  2. idle physical footprint and CPU, one window;
  3. physical footprint with 3 windows and 20 open files;
  4. native keyDown-to-Metal CPU submission p50 / p99 (not GPU completion or display scan-out).
     System Events types the keys: the terminal needs Privacy & Security → Automation
     (System Events) and → Accessibility; --skip-latency skips.
     The start is after native key-equivalent deduplication, before IME / GPUI dispatch;
     the end is the same window's next successful Metal commit return, with no extra frame.
     Keys coalesced into one frame share that endpoint. A key need not change pixels.
     Hardware / OS input delivery before keyDown is excluded. Old key_latency / frame_upper
     logs are not samples, and old reports are not directly comparable.
     The benchmark selects the already-enabled ABC input source and restores the previous
     native source even on failure. It verifies saved ASCII edits in the temporary fixture;
     the save key is separate from the measured keys. No IME preedit is counted as editing.

--breakdown also measures 3 windows without files and 1 window with the 20 files, and prints
the `footprint` categories of the 3-window, 20-file run, to tell windows from documents.

Footprint counts the whole process tree (git children included), as CLAUDE.md asks.

    cargo build --profile dist --locked
    python3 tools/measure_budget.py [--output report.json]
"""
import argparse
import ctypes
import json
import math
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

from sample_resources import mach_timebase
from native_input_source import abc_input_source

ROOT = Path(__file__).resolve().parents[1]

# (target, limit) from CLAUDE.md.
BUDGET = {
    "cold_start_ms": (250, 400),
    "idle_footprint_mb": (70, 100),
    "three_windows_20_docs_mb": (220, 300),
    "idle_cpu_p95_percent": (0.0, 1.0),
    "key_cpu_submit_p99_ms": (8.3, 16.7),
}

# The dist binary, stripped aarch64: ≤ 50 MB target, 52 MB limit (CLAUDE.md, CI checks the
# limit in .github/workflows/ci.yml as BINARY_BUDGET_BYTES).
BINARY_BUDGET = (50_000_000, 52_000_000)


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


class CpuMeter:
    """CPU percent over the real interval, from one rusage snapshot per process.

    The loop below cannot assume `time.sleep(interval)` took exactly `interval` seconds:
    `cpu_seconds` walks the process tree (which may take tens of milliseconds). Percent is
    therefore divided by the measured elapsed time, and both the startup window and the
    steady window are kept rather than dropping the early samples.
    """

    def __init__(self, pid):
        self.pid = pid
        self.previous = cpu_seconds(pid)
        self.previous_at = time.monotonic()

    def sample(self):
        now = cpu_seconds(self.pid)
        now_at = time.monotonic()
        elapsed = now_at - self.previous_at
        percent = (now - self.previous) / elapsed * 100 if elapsed > 0 else 0.0
        self.previous = now
        self.previous_at = now_at
        return percent, elapsed


def p95(samples):
    """The smallest sample at or above the 95th percentile (`ceil(0.95 n)`-th of the
    sorted samples, 1-based), clamped to the last one."""
    ordered = sorted(samples)
    if not ordered:
        return 0.0
    rank = max(1, -(-95 * len(ordered) // 100))  # ceil(0.95 n) without float rounding
    return ordered[min(len(ordered) - 1, rank - 1)]


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


def measure_idle(binary, folder, env, seconds, file=None, show_categories=False, steady_at=None):
    """One window idle; with `file` a document is shown, otherwise the welcome page.

    The budget window starts 10 s after the document/workspace finished loading and keeps
    every sample, as the budget asks. `steady_at` (diagnostic only; it never replaces the
    budget number) additionally measures a second window starting `steady_at` seconds after
    that same point, and is reported in the JSON.
    """
    # The frame log tells whether idle CPU comes from frames (a wake-up) or from other work.
    app = App(binary, [folder, *([file] if file else [])], dict(env, ZJ_FRAME_LOG="1"))
    try:
        app.wait_for("event=first_frame", timeout=15)
        app.wait_for("event=refresh_finished", timeout=30)
        if file:
            app.wait_for("event=document_opened", timeout=30)
        load_done = time.monotonic()
        time.sleep(10)
        frames_before = len(app.events("event=frame"))
        wakes_before = len(app.events("event=display_link state=woken"))
        logs = []

        def collect(window_seconds):
            footprints, cpu = [], []
            meter = CpuMeter(app.process.pid)
            end = time.monotonic() + window_seconds
            while time.monotonic() < end:
                time.sleep(0.2)
                percent, elapsed = meter.sample()
                cpu.append(percent)
                footprints.append(footprint_mb(app.process.pid))
                logs.append((percent, elapsed))
            return statistics.median(footprints), p95(cpu), cpu

        idle, cpu_p95, _ = collect(seconds)
        steady = None
        if steady_at is not None:
            # Anchored like the budget window: both offsets count from the end of loading.
            # The budget window runs first, so a later start cannot be earlier than now;
            # the recorded offset is the measured one, never the requested one.
            budget_start = load_done + 10
            target = budget_start + steady_at
            time.sleep(max(0.0, target - time.monotonic()))
            actual_start = time.monotonic() - budget_start
            steadies, steady_p95, _ = collect(seconds)
            steady = {
                "requested_after_load_seconds": 10 + steady_at,
                "actual_start_after_budget_start_seconds": actual_start,
                "seconds": seconds,
                "footprint_mb": steadies,
                "cpu_p95_percent": steady_p95,
            }
            print(f"    诊断（不替代预算窗口）：预算窗口起点后 {actual_start:.1f} s 起的 "
                  f"{seconds:.0f} s（实测，请求值 {steady_at:.0f} s）"
                  f"footprint {steadies:.1f} MB，CPU p95 {steady_p95:.3f}%")
            print(f"    采样间隔：中位 {statistics.median(e for _, e in logs):.3f} s，"
                  f"最大 {max(e for _, e in logs):.3f} s（按实测间隔计算百分比）")
        frames = len(app.events("event=frame")) - frames_before
        wakes = len(app.events("event=display_link state=woken")) - wakes_before
        print(f"    测量期间出帧 {frames} 次，display link 被唤醒 {wakes} 次"
              + ("" if frames or wakes else "（CPU 不是画帧花的）"))
        if show_categories:
            for dirty, name in categories(app.process.pid):
                print(f"    {dirty:8.1f} MB  {name}")
        return idle, cpu_p95, steady
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


def latency_summary(lines, keys):
    """Accept only complete samples with the CPU-submission boundary declared explicitly."""
    if keys <= 0:
        raise ValueError("keys must be positive")
    samples = []
    for line in lines:
        fields = dict(part.split("=", 1) for part in line.split() if "=" in part)
        if fields.get("event") == "key_latency_dropped":
            raise RuntimeError("key latency queue overflowed; run is invalid")
        if fields.get("event") != "key_cpu_submit":
            continue
        if (fields.get("start"), fields.get("end")) != ("native_key_down", "metal_commit_return"):
            raise RuntimeError("unexpected key latency measurement boundary")
        try:
            value = float(fields["us"]) / 1000
        except (KeyError, ValueError) as error:
            raise RuntimeError("invalid CPU submission sample") from error
        if not math.isfinite(value) or value < 0:
            raise RuntimeError("invalid CPU submission sample")
        samples.append(value)
    if len(samples) != keys:
        raise RuntimeError(f"expected {keys} CPU submission samples, got {len(samples)}; "
                           "old logs, missed input or extra keys cannot validate this run")
    samples.sort()
    p99 = samples[math.ceil(len(samples) * 0.99) - 1]
    return statistics.median(samples), p99, len(samples)


def latency_osascript(script, timeout=30):
    try:
        result = subprocess.run(["osascript", "-e", script], capture_output=True, text=True,
                                timeout=timeout)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("System Events 输入超时，本次按键延迟测量无效") from error
    if result.returncode:
        raise RuntimeError("System Events 操作失败，本次输入已中止："
                           + result.stderr.strip())


def verify_ascii_edit(before, after, keys):
    # Only synthetic benchmark files: preserve every non-injected byte.
    if len(after) != len(before) + keys or after.replace(b"a", b"") != before.replace(b"a", b""):
        raise RuntimeError(f"Expected {keys} committed ASCII characters in the saved fixture")


def measure_latency(binary, folder, file, env, keys):
    if keys <= 0:
        raise ValueError("keys must be positive")
    before = file.read_bytes()
    env = dict(env, ZJ_LATENCY_LOG="1")
    app = App(binary, [folder, file], env)
    try:
        app.wait_for("event=document_opened", timeout=30)
        time.sleep(2)
        pid = app.process.pid
        guard = f'''if (unix id of (first application process whose frontmost is true)) is not {pid} then
    error "ZJ lost foreground focus; input stopped"
end if'''
        latency_osascript(f'''tell application "System Events"
set frontmost of (first process whose unix id is {pid}) to true
delay 0.5
{guard}
end tell''')
        with abc_input_source() as source:
            source.require_abc()
            latency_osascript(f'''tell application "System Events"
repeat {keys} times
    {guard}
    keystroke "a"
    delay 0.03
end repeat
end tell''', timeout=max(30, keys * 0.2))
            time.sleep(1)
            source.require_abc()
            # Freeze samples before the separate save key generates another timing event.
            summary = latency_summary(app.events("event=key_"), keys)
            latency_osascript(f'''tell application "System Events"
{guard}
keystroke "s" using command down
end tell''')
            deadline = time.monotonic() + 5
            while file.read_bytes() == before and time.monotonic() < deadline:
                time.sleep(0.05)
            verify_ascii_edit(before, file.read_bytes(), keys)
            source.require_abc()
            return summary
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
    parser.add_argument("--steady-window", type=float, default=None,
                        help="diagnostic only: also measure a window this many seconds after the "
                             "budget window's start (end of loading + 10 s); must be at least "
                             "--idle-seconds. The JSON records the measured start in "
                             "idle_steady.actual_start_after_budget_start_seconds")
    parser.add_argument("--breakdown", action="store_true",
                        help="also 3 windows without files, 1 window with the files, categories")
    parser.add_argument("--per-window", action="store_true",
                        help="also what one more window costs, by footprint category")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.steady_window is not None and args.steady_window < args.idle_seconds:
        parser.error("--steady-window must be at least --idle-seconds: the diagnostic window "
                     "starts after the budget window, so a smaller offset would overlap it")
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
        # The budget's idle CPU is measured with a document shown (no logo cursor animation).
        idle, cpu, steady = measure_idle(args.binary, folders[0], env, args.idle_seconds,
                                         files[0], show_categories=args.breakdown,
                                         steady_at=args.steady_window)
        report["idle_footprint_mb"] = idle
        report["idle_cpu_p95_percent"] = cpu
        if steady:
            report["idle_steady"] = steady
        print(f"空闲（1 个窗口，打开一个文件）：{idle:.1f} MB；CPU p95：{cpu:.3f}%")
        welcome, blink_cpu, blink_steady = measure_idle(args.binary, folders[0], env,
                                                        args.idle_seconds,
                                                        show_categories=args.breakdown,
                                                        steady_at=args.steady_window)
        report["idle_welcome_footprint_mb"] = welcome
        report["idle_welcome_cpu_p95_percent"] = blink_cpu
        if blink_steady:
            report["idle_welcome_steady"] = blink_steady
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
                report["key_cpu_submit_p50_ms"] = p50
                report["key_cpu_submit_p99_ms"] = p99
                report["key_cpu_submit_samples"] = count
                report["key_cpu_submit_start"] = "native_key_down"
                report["key_cpu_submit_end"] = "metal_commit_return"
                report["key_cpu_submit_input_source"] = "com.apple.keylayout.ABC"
                report["key_cpu_submit_saved_ascii_keys"] = count
                print(f"原生按键到 CPU 提交（{count} 次，不含 GPU 完成／屏幕 scanout）："
                      f"p50 {p50:.2f} ms，p99 {p99:.2f} ms")
            except RuntimeError as error:
                print(f"按键延迟未测：{error}")

    print()
    print(f"{'指标':<24}{'实测':>10}{'目标':>10}{'上限':>10}  结论")
    for name, (target, limit) in BUDGET.items():
        if name in report:
            value = report[name]
            print(f"{name:<24}{value:>10.2f}{target:>10}{limit:>10}  {verdict(name, value)}")
    print(f"二进制体积：{report['binary_bytes'] / 1_000_000:.2f} MB"
          f"（目标 ≤ {BINARY_BUDGET[0] / 1_000_000:g} MB，上限 {BINARY_BUDGET[1] / 1_000_000:g} MB）")
    if args.output:
        args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
