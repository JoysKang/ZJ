#!/usr/bin/env python3
"""One native Chinese-input and crash-recovery case using only a temporary fixture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_native_shortcuts import Driver, MARKER, input_source, osascript, wait_for


def run(binary, output):
    output.mkdir(parents=True, exist_ok=False)
    result = {'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'input_source': input_source()}
    with tempfile.TemporaryDirectory(prefix='zj-recovery-') as temporary:
        base = Path(temporary)
        workspace = base / 'ws'
        workspace.mkdir()
        file = workspace / 'sample.rs'
        original = 'fn main() {}\n'
        file.write_text(original)
        (base / 'settings.json').write_text(json.dumps({'auto_save': 'off', 'auto_input_source': False, 'hidden_files': []}))
        env = dict(os.environ, ZJ_SETTINGS=str(base / 'settings.json'), ZJ_SESSION=str(base / 'session.json'), ZJ_AGENT_DB=str(base / 'agent.db'))
        app = None
        handles = []
        def launch(label):
            nonlocal app
            log_path = output / (label + '.log')
            log = log_path.open('w')
            handles.append(log)
            app = subprocess.Popen([str(binary), str(workspace), str(file)], env=env, stdout=log, stderr=log)
            result[label + '_pid'] = app.pid
            wait_for(lambda: 'event=document_opened' in log_path.read_text(), 'document open missing', 30)
            driver = Driver(app.pid)
            osascript(f'tell application "System Events" to set frontmost of {driver.process} to true')
            wait_for(lambda: driver.query('get frontmost') == 'true', 'activation failed')
            driver.action('set position of window 1 to {60, 60}\nset size of window 1 to {1280, 800}\nclick at {700, 300}')
            return driver
        try:
            driver = launch('before_crash')
            if result['input_source'] == 'com.tencent.inputmethod.wetype.pinyin':
                driver.action('keystroke "nihao"')
                preedit = driver.settled_text()
                driver.action('key code 49')
                chinese = driver.settled_text()
                assert any('一' <= char <= '鿿' for char in chinese), 'candidate did not commit Chinese text'
                driver.action('key code 53')
                assert driver.settled_text() == chinese, 'Escape cancelled candidate instead of preserving commit'
                driver.action('keystroke "z" using command down')
                wait_for(lambda: driver.text() == original, 'undo did not restore original text')
                result['chinese_candidate_commit_undo'] = True
                result['candidate_text'] = chinese.removesuffix(original)
                result['preedit_visible'] = preedit != original
            else:
                result['chinese_candidate_commit_undo'] = 'not tested: current source is not WeType Pinyin'
            driver.action(f'keystroke "{MARKER}"')
            committed = driver.commit_text()
            def snapshots():
                return list((base / 'recovery').glob('*.json'))
            wait_for(lambda: any(MARKER in path.read_text() for path in snapshots()), 'real recovery snapshot missing', 10)
            result['snapshot_count'] = len(snapshots())
            assert file.read_text() == original, 'unsaved input reached original file'
            app.kill()
            app.wait(timeout=5)
            result['crash_exit_code'] = app.returncode
            driver = launch('after_crash')
            wait_for(lambda: driver.text() == committed, 'recovered editor content differs', 10)
            assert file.read_text() == original, 'recovery overwrote original file'
            result['recovery_text_exact'] = True
            result['original_file_unchanged'] = True
            result['input_source_after'] = input_source()
            result['recovery_passed'] = True
            result['passed'] = result['chinese_candidate_commit_undo'] is True
        except Exception as error:
            result['error'] = str(error)
            raise
        finally:
            if app is not None and app.poll() is None:
                app.terminate()
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
            for handle in handles:
                handle.close()
            (output / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')
            print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    run(args.binary.resolve(strict=True), args.output.resolve())
