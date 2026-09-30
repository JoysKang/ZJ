#!/usr/bin/env python3
"""Check Git deadlines when descendants keep stdout/stderr open.

Run after a Release build. Compiles the current service in a temporary directory;
uses a fake Git and never accesses a real repository.
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path


project = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1]
dependencies = project / "target/release/deps"
libraries = list(dependencies.glob("liblibc-*.rlib"))
if not libraries:
    raise SystemExit("Run cargo build --release --locked before this check")
libc = max(libraries, key=lambda path: path.stat().st_mtime_ns)

with tempfile.TemporaryDirectory(prefix="zj-check-deadline-") as temporary:
    root = Path(temporary)
    subprocess.run([
        "rustc", "--edition=2024", "--crate-name", "workspace_editor_core",
        "--crate-type", "rlib", str(project / "crates/core/src/lib.rs"),
        "-o", str(root / "libworkspace_editor_core.rlib"),
    ], check=True)
    subprocess.run([
        "rustc", "--edition=2024", "--crate-name", "workspace_editor_git",
        "--crate-type", "rlib", str(project / "crates/git_service/src/lib.rs"),
        "--extern", f"workspace_editor_core={root}/libworkspace_editor_core.rlib",
        "--extern", f"libc={libc}", "-L", str(root), "-L", str(dependencies),
        "-o", str(root / "libworkspace_editor_git.rlib"),
    ], check=True)
    harness = root / "harness.rs"
    harness.write_text('''use std::{path::Path, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};
use workspace_editor_git::GitService;
fn main() {
    let cancelling = std::env::args().nth(1).as_deref() == Some("cancel");
    let service = GitService::new(1, if cancelling { Duration::from_secs(10) } else { Duration::from_millis(1000) }).unwrap();
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        if cancelling { let cancel = &cancel; scope.spawn(move || { std::thread::sleep(Duration::from_millis(1000)); cancel.store(true, Ordering::Relaxed); }); }
        service.identify(Path::new("/tmp"), &cancel)
    });
    let elapsed = started.elapsed();
    let kind = result.unwrap_err().kind();
    println!("elapsed={:.3}s kind={kind:?}", elapsed.as_secs_f64());
    assert!(elapsed < Duration::from_millis(1700), "descendant delayed deadline");
    assert_eq!(kind, if cancelling { std::io::ErrorKind::Interrupted } else { std::io::ErrorKind::TimedOut });
}''')
    subprocess.run([
        "rustc", "--edition=2024", str(harness),
        "--extern", f"workspace_editor_git={root}/libworkspace_editor_git.rlib",
        "-L", str(root), "-L", str(dependencies), "-o", str(root / "harness"),
    ], check=True)
    fake = root / "git"
    fake.write_text(f'''#!{sys.executable}
import os, pathlib, subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(3)'])
pathlib.Path(os.environ['ZJ_CHECK_CHILD_PID']).write_text(str(child.pid))
if os.environ.get('ZJ_CHECK_GIT_MODE') == 'parent_exits':
    os._exit(0)
time.sleep(10)
''')
    fake.chmod(0o755)
    for mode in ("timeout", "parent_exits", "cancel"):
        marker = root / f"{mode}.pid"
        env = {
            **os.environ, "PATH": f'{root}:{os.environ["PATH"]}',
            "ZJ_CHECK_CHILD_PID": str(marker), "ZJ_CHECK_GIT_MODE": mode,
        }
        result = subprocess.run(
            [str(root / "harness"), mode], env=env,
            capture_output=True, text=True, timeout=3,
        )
        print(f"{mode}: {result.stdout.strip()}")
        if result.returncode:
            print(result.stderr)
            raise SystemExit(result.returncode)
        assert marker.exists(), "fake Git failed to spawn descendant before deadline"
        try:
            os.kill(int(marker.read_text()), 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError("descendant PID still exists after cancellation")
    print("PASS: timeout, exited parent, cancellation; no live descendant remains")
