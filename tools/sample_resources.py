#!/usr/bin/env python3
"""Sample macOS physical footprint and CPU of one application process tree."""
import argparse
import csv
import ctypes
import json
import math
import statistics
import subprocess
import time
from pathlib import Path


class RUsage(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(name, ctypes.c_uint64) for name in (
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins", "wired_size",
        "resident_size", "phys_footprint", "proc_start_abstime", "proc_exit_abstime",
        "child_user_time", "child_system_time", "child_pkg_idle_wkups", "child_interrupt_wkups",
        "child_pageins", "child_elapsed_abstime", "diskio_bytesread", "diskio_byteswritten")]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--interval", type=float, default=0.2)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.seconds <= 0 or not 0.1 <= args.interval <= 0.25:
        parser.error("seconds must be positive; interval must be 0.1..0.25")
    lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    lib.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    lib.proc_pid_rusage.restype = ctypes.c_int
    samples, overhead, previous, totals, cpu_samples = [], [], {}, [], []
    unavailable_children = 0
    started = time.monotonic()
    with args.output.open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["elapsed_seconds", "pid", "ppid", "footprint_bytes", "rss_bytes", "cpu_one_core_percent", "sample_cost_seconds"])
        while time.monotonic() - started < args.seconds:
            tick = time.monotonic()
            result = subprocess.run(["ps", "-axo", "pid=,ppid="], capture_output=True, text=True, timeout=5, check=True)
            parents = {int(p): int(pp) for p, pp in (line.split() for line in result.stdout.splitlines())}
            if args.pid not in parents:
                break
            members = {args.pid}
            while True:
                children = {p for p, pp in parents.items() if pp in members}
                if children <= members:
                    break
                members |= children
            snapshot = []
            for pid in sorted(members):
                usage = RUsage()
                if lib.proc_pid_rusage(pid, 2, ctypes.byref(usage)):
                    if pid == args.pid:
                        raise OSError(ctypes.get_errno(), f"proc_pid_rusage failed for app pid {pid}")
                    unavailable_children += 1
                    continue
                cpu_time = usage.user_time + usage.system_time
                identity = (pid, usage.proc_start_abstime)
                before = previous.get(identity)
                cpu = (cpu_time - before[1]) / 1e9 / (tick - before[0]) * 100 if before else 0
                previous[identity] = (tick, cpu_time)
                snapshot.append((pid, usage.phys_footprint, usage.resident_size, cpu))
            cost = time.monotonic() - tick
            elapsed = tick - started
            for pid, footprint, rss, cpu in snapshot:
                writer.writerow([round(elapsed, 6), pid, parents[pid], footprint, rss, round(cpu, 4), round(cost, 6)])
            stream.flush()
            totals.append(sum(row[1] for row in snapshot))
            if len(totals) > 1:
                cpu_samples.append(sum(row[3] for row in snapshot))
            overhead.append(cost)
            samples.append(elapsed)
            time.sleep(max(0, args.interval - cost))
    if not totals:
        raise RuntimeError("No samples: process was not running")
    p95 = lambda values: sorted(values)[max(0, math.ceil(len(values) * 0.95) - 1)]
    gaps = [b - a for a, b in zip(samples, samples[1:])]
    report = {"version": 1, "metric": "simultaneous_process_tree_physical_footprint", "unit": "decimal bytes",
              "samples": len(totals), "unavailable_child_samples": unavailable_children,
              "duration_seconds": round(samples[-1], 3),
              "footprint_median": statistics.median(totals), "footprint_p95": p95(totals), "footprint_max": max(totals),
              "cpu_p95_one_core_percent": p95(cpu_samples) if cpu_samples else None,
              "sample_cost_p95_seconds": p95(overhead), "interval_p95_seconds": p95(gaps) if gaps else None,
              "limitations": ["Short-lived children between samples can be missed", "No GPU command or draw instrumentation",
                              "No theoretical peak guarantee", "First observation of each child has no CPU delta"]}
    args.output.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
