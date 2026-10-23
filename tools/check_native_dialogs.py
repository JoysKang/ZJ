#!/usr/bin/env python3
"""Bounded native dialog checks. Reserve exclusive GUI use before running."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_native_shortcuts import Driver, MARKER, osascript, wait_for


def run(binary, output):
    output.mkdir(parents=True, exist_ok=False)
    result = {"sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
    with tempfile.TemporaryDirectory(prefix="zj-dialogs-") as temporary:
        base = Path(temporary)
        workspace = base / base.name
        workspace.mkdir()
        document = workspace / "sample.rs"
        original = "fn main() {}\n"
        document.write_text(original)
        (base / "settings.json").write_text(json.dumps({
            "auto_save": "off", "auto_input_source": False, "hidden_files": [],
        }))
        env = dict(os.environ, ZJ_SETTINGS=str(base / "settings.json"),
                   ZJ_SESSION=str(base / "session.json"), ZJ_AGENT_DB=str(base / "agent.db"))
        log_path = output / "app.log"
        with log_path.open("w") as log:
            app = subprocess.Popen([str(binary), str(workspace), str(document)],
                                   env=env, stdout=log, stderr=log)
            result["pid"] = app.pid
            print(f"probe_pid={app.pid}", flush=True)
            driver = Driver(app.pid)
            try:
                wait_for(lambda: "event=document_opened" in log_path.read_text(), "document did not open", 30)
                osascript(f'tell application "System Events" to set frontmost of {driver.process} to true')
                wait_for(lambda: driver.query('get frontmost') == 'true', 'activation failed')
                driver.action('set position of window 1 to {60, 60}\nset size of window 1 to {1280, 800}\nclick at {700, 300}')
                driver.action(f'keystroke "{MARKER}"')
                committed = driver.commit_text()
                assert document.read_text() == original
                driver.action('keystroke "w" using command down')
                wait_for(lambda: driver.query('get count of sheets of window 1') == '1', 'close prompt missing')
                result['close_prompt'] = driver.query('get name of every button of sheet 1 of window 1')
                driver.action('click button "取消" of sheet 1 of window 1')
                wait_for(lambda: driver.query('get count of sheets of window 1') == '0', 'cancel did not dismiss')
                assert driver.text() == committed and document.read_text() == original
                result['cancel_preserves_buffer_and_disk'] = True
                driver.action('keystroke "w" using command down')
                wait_for(lambda: driver.query('get count of sheets of window 1') == '1', 'second close prompt missing')
                driver.action('click button "保存" of sheet 1 of window 1')
                wait_for(lambda: document.read_text() == committed, 'close-save did not write exact content')
                wait_for(lambda: driver.query('get count of text areas of group 1 of window 1') == '0', 'saved document did not close')
                result['save_writes_then_closes'] = True
                driver.action('click menu item "新建文件" of menu 1 of menu bar item "文件" of menu bar 1')
                wait_for(lambda: driver.query('get count of text areas of group 1 of window 1') == '1', 'Untitled missing')
                driver.action(f'keystroke "{MARKER}"')
                committed = driver.commit_text()
                driver.action('keystroke "s" using command down')
                wait_for(lambda: driver.query('get count of windows whose name is "Save"') == '1', 'Save panel missing')
                result['native_save_panel'] = True
                driver.action('keystroke "g" using {command down, shift down}')
                wait_for(lambda: driver.query('get count of sheets of window "Save"') == '1', 'Go to folder missing')
                driver.action(f'set value of text field 1 of sheet 1 of window "Save" to "{workspace}/"')
                assert driver.query('get value of text field 1 of sheet 1 of window "Save"') == str(workspace) + '/'
                driver.action('key code 36')
                wait_for(lambda: driver.query('get count of sheets of window "Save"') == '0', 'Go to folder did not finish')
                driver.action('set value of text field "Save As:" of splitter group 1 of window "Save" to "untitled-saved.txt"')
                result['save_location'] = driver.query('get value of pop up button "Where:" of splitter group 1 of window "Save"')
                assert result['save_location'] == workspace.name, 'save destination is not unique fixture workspace'
                assert driver.query('get value of text field "Save As:" of splitter group 1 of window "Save"') == 'untitled-saved.txt'
                driver.action('click button "Save" of splitter group 1 of window "Save"')
                target = workspace / 'untitled-saved.txt'
                wait_for(lambda: target.exists() and target.read_text() == committed, 'Untitled save did not write exact content')
                result['untitled_saved_exactly'] = True
                result['passed'] = True
            except Exception as error:
                result['error'] = str(error)
                raise
            finally:
                app.terminate()
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
                result['exit_code'] = app.returncode
                (output / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
                print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    run(args.binary.resolve(strict=True), args.output.resolve())
