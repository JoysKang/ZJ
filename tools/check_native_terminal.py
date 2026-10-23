#!/usr/bin/env python3
"""Bounded terminal smoke test in a temporary workspace; reserve GUI first."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_native_shortcuts import Driver, input_source, osascript, wait_for


def children(pid):
    result = subprocess.run(['pgrep', '-P', str(pid)], capture_output=True, text=True)
    return [int(value) for value in result.stdout.split()]


def run(binary, output):
    output.mkdir(parents=True, exist_ok=False)
    result = {'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'input_source': input_source()}
    if result['input_source'] != 'com.apple.keylayout.ABC':
        raise RuntimeError('Run this shell-input case inside native_input_source.abc_input_source()')
    with tempfile.TemporaryDirectory(prefix='zj-terminal-') as temporary:
        base = Path(temporary)
        workspace = base / 'ws'
        workspace.mkdir()
        document = workspace / 'sample.rs'
        document.write_text('fn main() {}\n')
        (base / 'settings.json').write_text(json.dumps({
            'auto_save': 'off', 'auto_input_source': False, 'hidden_files': [],
        }))
        env = dict(os.environ, ZJ_SETTINGS=str(base / 'settings.json'),
                   ZJ_SESSION=str(base / 'session.json'), ZJ_AGENT_DB=str(base / 'agent.db'),
                   SHELL='/bin/sh', ENV='/dev/null')
        log_path = output / 'app.log'
        with log_path.open('w') as log:
            app = subprocess.Popen([str(binary), str(workspace), str(document)],
                                   env=env, stdout=log, stderr=log)
            result['pid'] = app.pid
            driver = Driver(app.pid)
            try:
                wait_for(lambda: 'event=document_opened' in log_path.read_text(), 'document did not open', 30)
                osascript(f'tell application "System Events" to set frontmost of {driver.process} to true')
                wait_for(lambda: driver.query('get frontmost') == 'true', 'activation failed')
                driver.action('set position of window 1 to {60, 60}\nset size of window 1 to {1280, 800}')
                driver.action('key code 50 using control down')
                wait_for(lambda: len(children(app.pid)) == 1, 'first shell missing')
                result['initial_shells'] = children(app.pid)
                for label in ('新建终端', '拆分终端', '终止终端', '隐藏面板'):
                    wait_for(lambda: driver.query(f'get count of (buttons of group 1 of window 1 whose name starts with "{label}")') == '1', f'missing or ambiguous terminal label: {label}')
                result['accessible_names'] = True
                driver.action('click (first button of group 1 of window 1 whose name starts with "隐藏面板")')
                wait_for(lambda: driver.query('get count of (buttons of group 1 of window 1 whose name starts with "新建终端")') == '0', 'panel did not hide')
                assert children(app.pid) == result['initial_shells'], 'hide changed shell ownership'
                driver.action('key code 50 using control down')
                wait_for(lambda: driver.query('get count of (buttons of group 1 of window 1 whose name starts with "新建终端")') == '1', 'panel did not reopen')
                result['hide_and_reopen'] = True
                command = f'printf TERMINALOK > {workspace}/terminal-marker.txt; pwd > {workspace}/terminal-cwd.txt'
                driver.action(f'keystroke "{command}"')
                driver.action('key code 36')
                time.sleep(.2)
                driver.action('key code 36')
                marker = workspace / 'terminal-marker.txt'
                wait_for(lambda: marker.exists() and marker.read_text() == 'TERMINALOK', 'terminal input did not execute in workspace')
                cwd_file = workspace / 'terminal-cwd.txt'
                wait_for(lambda: cwd_file.exists(), 'terminal did not report working directory')
                assert Path(cwd_file.read_text().strip()).resolve() == workspace.resolve(), 'terminal cwd differs'
                result['input_and_working_directory'] = True
                driver.action('click (first button of group 1 of window 1 whose name starts with "新建终端")')
                wait_for(lambda: len(children(app.pid)) == 2, 'new terminal did not start shell')
                result['new_terminal'] = True
                driver.action('click (first button of group 1 of window 1 whose name starts with "拆分终端")')
                wait_for(lambda: len(children(app.pid)) == 3, 'split did not start shell')
                result['split_terminal'] = True
                for expected in (2, 1, 0):
                    driver.action('click button "终止终端" of group 1 of window 1')
                    wait_for(lambda: len(children(app.pid)) == expected, 'terminal close did not reap shell')
                result['close_reaps_all_shells'] = True
                result['passed'] = True
            except Exception as error:
                result['error'] = str(error)
                raise
            finally:
                owned = children(app.pid)
                app.terminate()
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
                result['exit_code'] = app.returncode
                result['children_before_cleanup'] = owned
                def alive(pid):
                    return subprocess.run(['ps', '-p', str(pid), '-o', 'pid='], capture_output=True).returncode == 0
                for _ in range(20):
                    remaining = [pid for pid in owned if alive(pid)]
                    if not remaining:
                        break
                    time.sleep(.05)
                result['children_after_cleanup'] = remaining
                if remaining or app.returncode not in (0, -15):
                    result['passed'] = False
                    result['cleanup_error'] = 'test process or shell cleanup incomplete'
                (output / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
                print(json.dumps(result, indent=2, ensure_ascii=False))
        if result.get('cleanup_error'):
            raise RuntimeError(result['cleanup_error'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    run(args.binary.resolve(strict=True), args.output.resolve())
