#!/usr/bin/env python3
"""Check Git deadlines when descendants keep stdout/stderr open.

Run after a `--profile dist` or `--release` build (it borrows that build's libc rlib;
CARGO_TARGET_DIR is honoured). Compiles the current service in a temporary directory;
uses a fake Git and never accesses a real repository.
"""

import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path


project = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1]
target = Path(os.environ.get("CARGO_TARGET_DIR") or project / "target")
if not target.is_absolute():
    target = project / target
# The newest libc rlib from either optimized profile.
candidates = [
    library
    for profile in ("dist", "release")
    for library in (target / profile / "deps").glob("liblibc-*.rlib")
]
if not candidates:
    raise SystemExit(
        f"No libc rlib in {target}/dist/deps or {target}/release/deps; "
        "run cargo build --profile dist --locked (or --release) before this check"
    )
libc = max(candidates, key=lambda path: path.stat().st_mtime_ns)
dependencies = libc.parent


def alive(pid):
    """Whether `pid` is a running process; a zombie (exited, not yet reaped) counts as dead."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    if Path("/proc/self/stat").exists():  # Linux
        try:
            # Field 3 of /proc/<pid>/stat, after the parenthesised command name.
            state = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
        except FileNotFoundError:
            return False
    else:  # macOS
        result = subprocess.run(
            ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True
        )
        state = result.stdout.strip()
        if result.returncode or not state:
            return False
    return not state.startswith("Z")

with tempfile.TemporaryDirectory(prefix="zj-check-deadline-") as temporary:
    root = Path(temporary)
    # The dist libc uses panic=abort; the standalone harness must use it too.
    subprocess.run([
        "rustc", "-C", "panic=abort", "--edition=2024", "--crate-name", "workspace_editor_core",
        "--crate-type", "rlib", str(project / "crates/core/src/lib.rs"),
        "-o", str(root / "libworkspace_editor_core.rlib"),
    ], check=True)
    subprocess.run([
        "rustc", "-C", "panic=abort", "--edition=2024", "--crate-name", "workspace_editor_git",
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
        "rustc", "-C", "panic=abort", "--edition=2024", str(harness),
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
        # SIGKILL to the group is delivered asynchronously; give the orphan a moment to die.
        descendant = int(marker.read_text())
        deadline = time.monotonic() + 0.5
        while alive(descendant) and time.monotonic() < deadline:
            time.sleep(0.01)
        if alive(descendant):
            raise AssertionError("descendant PID still exists after cancellation")
    print("PASS: timeout, exited parent, cancellation; no live descendant remains")
