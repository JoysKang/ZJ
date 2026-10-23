#!/usr/bin/env python3
"""Regression tests for the latency budget's measurement boundary (no native UI)."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
from contextlib import contextmanager
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
spec = importlib.util.spec_from_file_location("measure_budget", Path(__file__).with_name("measure_budget.py"))
budget = importlib.util.module_from_spec(spec)
spec.loader.exec_module(budget)


def sample(us):
    return f"event=key_cpu_submit us={us} start=native_key_down end=metal_commit_return"


class LatencyTests(unittest.TestCase):
    def test_submission_samples_and_nearest_rank_p99(self):
        lines = [sample(i * 1000) for i in range(1, 101)]
        lines += ["event=key_latency us=1", "event=diff_timing stage=frame_upper ms=1"]
        self.assertEqual(budget.latency_summary(lines, 100), (50.5, 99, 100))
        self.assertIn("key_cpu_submit_p99_ms", budget.BUDGET)
        self.assertNotIn("key_latency_p99_ms", budget.BUDGET)

    def test_old_or_upper_bound_logs_cannot_satisfy_sample_count(self):
        for line in ("event=key_latency us=1", "event=diff_timing stage=frame_upper ms=1",
                     "event=key_cpu_submit_extra us=1"):
            with self.subTest(line=line), self.assertRaises(RuntimeError):
                budget.latency_summary([line], 1)

    def test_incomplete_extra_and_overflow_samples_are_rejected(self):
        for lines in ([], [sample(1)] * 2,
                      [sample(1), "event=key_latency_dropped reason=pending_full"]):
            with self.subTest(lines=lines), self.assertRaises(RuntimeError):
                budget.latency_summary(lines, 1)

    def test_wrong_boundary_and_invalid_values_are_rejected(self):
        for line in (sample(1).replace("metal_commit_return", "frame_upper"),
                     sample(1).replace("native_key_down", "intercept_keystrokes"),
                     "event=key_cpu_submit us=1", sample("nan"), sample("inf"),
                     sample(-1), sample("bad"), sample(1).replace("us=1", "")):
            with self.subTest(line=line), self.assertRaises(RuntimeError):
                budget.latency_summary([line], 1)

    def test_positive_keys_and_single_sample(self):
        with self.assertRaises(ValueError):
            budget.latency_summary([], 0)
        self.assertEqual(budget.latency_summary([sample(1234)], 1), (1.234, 1.234, 1))

    def test_script_failure_keeps_real_cause_and_timeout_is_invalid(self):
        result = budget.subprocess.CompletedProcess([], 1, stderr="ZJ lost foreground focus; input stopped")
        with patch.object(budget.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "lost foreground focus") as error:
                budget.latency_osascript("test")
            self.assertNotIn("权限", str(error.exception))
        with patch.object(budget.subprocess, "run", side_effect=budget.subprocess.TimeoutExpired("osascript", 30)):
            with self.assertRaisesRegex(RuntimeError, "测量无效"):
                budget.latency_osascript("test")

    def test_saved_edit_must_preserve_non_injected_bytes(self):
        budget.verify_ascii_edit(b"fn main() {}", b"aaafn main() {}", 3)
        for after in (b"fn main() {}", b"zzzfn main() {}", b"aaafn main() {x"):
            with self.subTest(after=after), self.assertRaises(RuntimeError):
                budget.verify_ascii_edit(b"fn main() {}", after, 3)

    def test_activation_losing_focus_never_switches_input_source(self):
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "test.rs"
            file.write_bytes(b"fn main() {}")
            with patch.object(budget, "App") as factory, \
                 patch.object(budget, "abc_input_source") as source, \
                 patch.object(budget, "latency_osascript", side_effect=RuntimeError("lost focus")) as osa, \
                 patch.object(budget.time, "sleep"):
                factory.return_value.process.pid = 123
                with self.assertRaisesRegex(RuntimeError, "lost focus"):
                    budget.measure_latency(None, Path(directory), file, {}, 2)
                source.assert_not_called()
                factory.return_value.stop.assert_called_once()
                self.assertIn('(unix id of (first application process whose frontmost is true)) is not 123',
                              osa.call_args.args[0])

    def test_save_event_is_excluded_and_failure_restores_and_stops(self):
        for fail_save in (False, True):
            state = []
            class Source:
                def require_abc(self): state.append("check_abc")
            @contextmanager
            def source():
                try:
                    yield Source()
                finally:
                    state.append("restored")
            class App:
                def __init__(self, *_):
                    self.process = type("Process", (), {"pid": 123})()
                    self.lines = [sample(1000)] * 2
                def wait_for(self, *_args, **_kwargs): pass
                def events(self, _): return list(self.lines)
                def stop(self): state.append("stopped")
            app = App()
            with tempfile.TemporaryDirectory() as directory:
                file = Path(directory) / "test.rs"
                file.write_bytes(b"fn main() {}")
                def osa(script, **_):
                    if 'keystroke' in script:
                        self.assertIn('frontmost', script)
                        self.assertIn('123', script)
                    if 'keystroke "s"' in script:
                        app.lines.append(sample(999999))
                        if fail_save:
                            raise RuntimeError("save failed")
                        file.write_bytes(b"aafn main() {}")
                with patch.object(budget, "App", return_value=app), \
                     patch.object(budget, "abc_input_source", source), \
                     patch.object(budget, "latency_osascript", side_effect=osa), \
                     patch.object(budget.time, "sleep"):
                    if fail_save:
                        with self.assertRaisesRegex(RuntimeError, "save failed"):
                            budget.measure_latency(None, Path(directory), file, {}, 2)
                    else:
                        self.assertEqual(budget.measure_latency(None, Path(directory), file, {}, 2),
                                         (1.0, 1.0, 2))
                self.assertEqual(state[-2:], ["restored", "stopped"])


if __name__ == "__main__":
    unittest.main()
