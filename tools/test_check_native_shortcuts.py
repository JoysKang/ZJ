"""Checks for the preedit false positive that invalidated native shortcut testing."""

import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "check_native_shortcuts", Path(__file__).with_name("check_native_shortcuts.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class CommitCheckTests(unittest.TestCase):
    def test_cancelled_preedit_is_not_committed_input(self):
        with self.assertRaisesRegex(AssertionError, "Input not committed"):
            probe.require_committed(probe.MARKER + "fixture", "fixture")

    def test_missing_marker_cannot_pass_unchanged_value(self):
        with self.assertRaisesRegex(AssertionError, "marker is missing"):
            probe.require_committed("fixture", "fixture")

    def test_committed_value_survives_escape(self):
        text = probe.MARKER + "fixture\n"
        probe.require_committed(text, text)


if __name__ == "__main__":
    unittest.main()
