#!/usr/bin/env python3
"""Check macOS shortcuts in an isolated ZJ window; reserve GUI use before running.

Requires Accessibility and System Events automation permission. Does not change the
input source. Each injected action checks the foreground PID; losing it aborts the
run. AX text may include IME preedit: Return commits input, and Escape must leave
that committed value unchanged before a save assertion is meaningful.
"""

import argparse
import sys
import hashlib
import json
import os
import signal
from pathlib import Path
import subprocess
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_input_source import InputSources


MARKER = "SHORTCUTPROBE"


def osascript(source):
    result = subprocess.run(["osascript", "-e", source], capture_output=True,
                            text=True, timeout=10)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return result.stdout.removesuffix("\n")


def input_source():
    """Read the selected input mode, not the keyboard-layout defaults cache."""
    return InputSources().current()


class Driver:
    def __init__(self, pid):
        self.pid = pid
        self.process = f"(first process whose unix id is {pid})"

    def query(self, command):
        return osascript(f'tell application "System Events" to tell {self.process} to {command}')

    def action(self, command):
        # Process-scoped System Events keystrokes still go to the foreground app.
        return osascript(f'''tell application "System Events"
if (unix id of first application process whose frontmost is true) is not {self.pid} then error "lost foreground"
tell {self.process}
{command}
end tell
end tell''')

    def text(self):
        return self.query('get value of text area 1 of group 1 of window 1')

    def focus(self):
        # AXFocusedUIElement belongs to the application, not its window.
        return self.query('get role of value of attribute "AXFocusedUIElement"')

    def settled_text(self):
        # Stability only synchronizes AX reads; it does not prove marked text is absent.
        previous = None
        stable_since = time.monotonic()
        latest = None

        def stable():
            nonlocal previous, stable_since, latest
            latest = self.text()
            if latest != previous:
                previous = latest
                stable_since = time.monotonic()
            return time.monotonic() - stable_since >= .25

        wait_for(stable, "AX text did not settle")
        return latest

    def commit_text(self):
        self.action('key code 36')
        committed = self.settled_text()
        self.action('key code 53')
        require_committed(committed, self.settled_text())
        return committed


def wait_for(predicate, description, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError(description)


def require_committed(before, after):
    if MARKER not in before or before != after:
        raise AssertionError("Input not committed: Escape changed the AX value or marker is missing")


def run(binary, output):
    output.mkdir(parents=True, exist_ok=False)
    result = {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    log_path = output / "app.log"
    try:
        with tempfile.TemporaryDirectory(prefix="zj-shortcuts-") as temporary:
            base = Path(temporary)
            workspace = base / "ws"
            workspace.mkdir()
            document = workspace / "sample.rs"
            original = 'fn main() {\n    println!("fixture");\n}\n'
            document.write_text(original)
            (base / "settings.json").write_text(json.dumps({
                "auto_input_source": False, "auto_save": "off", "hidden_files": [],
            }))
            env = dict(os.environ, ZJ_SETTINGS=str(base / "settings.json"),
                       ZJ_SESSION=str(base / "session.json"), ZJ_AGENT_DB=str(base / "agent.db"))
            with log_path.open("w") as log:
                app = subprocess.Popen([str(binary), str(workspace), str(document)],
                                       env=env, stdout=log, stderr=log)
                result["pid"] = app.pid
                print(f"probe_pid={app.pid}", flush=True)
                driver = Driver(app.pid)
                try:
                    wait_for(lambda: "event=document_opened" in log_path.read_text(),
                             "document did not open", timeout=30)
                    osascript(f'tell application "System Events" to set frontmost of {driver.process} to true')
                    wait_for(lambda: driver.query("get frontmost") == "true", "activation failed")
                    driver.action('set position of window 1 to {60, 60}\nset size of window 1 to {1280, 800}')
                    driver.action('click at {700, 300}')
                    wait_for(lambda: driver.query('get count of text areas of group 1 of window 1') == "1",
                             "editor AX text area missing")
                    result["ax_focus"] = driver.focus()
                    result["input_source"] = input_source()
                    driver.action(f'keystroke "{MARKER}"')
                    wait_for(lambda: MARKER in driver.text(), "injected text missing from AX")
                    # This observation alone is not a failure: the marker may be preedit.
                    driver.action('key code 1 using command down')
                    time.sleep(.3)
                    result["saved_before_commit"] = MARKER in document.read_text()
                    committed = driver.commit_text()
                    result["commit_survives_escape"] = True
                    if document.read_text() == committed:
                        # The pre-commit observation may already have saved on an ASCII source.
                        committed = driver.commit_text()
                    assert document.read_text() != committed, "no unsaved input to test"
                    driver.action('key code 1 using command down')
                    wait_for(lambda: document.read_text() == committed,
                             "physical Command-S did not save committed input")
                    result["keycode_save"] = True
                    first_save = document.read_text()
                    driver.action('keystroke "SECOND"')
                    committed = driver.commit_text()
                    assert "SECOND" in committed, "second marker missing"
                    assert document.read_text() == first_save, "fixture saved without Command-S"
                    driver.action('keystroke "s" using command down')
                    wait_for(lambda: document.read_text() == committed,
                             "keystroke Command-S did not save committed input")
                    result["keystroke_save"] = True
                    assert driver.query('get count of text fields of group 1 of window 1') == "0", "unexpected input field before Command-P"
                    driver.action('keystroke "p" using command down')
                    wait_for(lambda: driver.query('get count of text fields of group 1 of window 1') == "1",
                             "Command-P did not open quick search")
                    result["quick_open"] = True
                    driver.action('key code 53')
                    wait_for(lambda: driver.query('get count of text fields of group 1 of window 1') == "0",
                             "quick search did not close")
                    driver.action('click at {700, 300}')
                    driver.action('keystroke "," using command down')
                    wait_for(lambda: driver.text() == (base / "settings.json").read_text(),
                             "Command-comma did not open isolated settings")
                    result["settings"] = True
                    driver.action('click at {700, 300}')
                    driver.action('keystroke "w" using command down')
                    wait_for(lambda: MARKER in driver.text(), "Command-W did not return to document")
                    result["close_settings"] = True
                    result["input_source_after"] = input_source()
                    assert result["input_source_after"] == result["input_source"], "input source changed during run"
                finally:
                    app.terminate()
                    try:
                        app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        app.kill()
                        app.wait()
                    result["exit_code"] = app.returncode
                assert app.returncode in (0, -signal.SIGTERM), "test process crashed or needed SIGKILL"
                result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
        raise
    finally:
        (output / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
        print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path, help="new evidence directory (must not exist)")
    args = parser.parse_args()
    run(args.binary.resolve(strict=True), args.output.resolve())
