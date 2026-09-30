#!/usr/bin/env python3
"""Read-only anonymous filesystem/Git counts; never reads source file contents."""
import argparse
import collections
import json
import os
import subprocess
import time
from pathlib import Path

EXCLUDED = {".git", "node_modules", "target", ".venv", "__pycache__"}


def git(root, *args):
    result = subprocess.run(["git", "--no-optional-locks", "-c", "core.fsmonitor=false", "-C", str(root), *args],
                            capture_output=True, timeout=30,
                            env={**{key: value for key, value in os.environ.items() if not key.startswith("GIT_")}, "GIT_TERMINAL_PROMPT": "0"})
    if result.returncode:
        raise RuntimeError(f"git failed ({result.returncode})")
    return result.stdout


def profile(root, identities):
    result = dict(worktrees=0, tracked=0, untracked=0, directories=0, max_depth=0,
                  tracked_bytes_on_disk=0, largest_tracked_file=0, index_bytes=0,
                  excluded_directories=0, symlinks_skipped=0, errors=0, error_categories={})
    queue = collections.deque([(root.resolve(), 0)])
    seen = set()
    while queue:
        folder, depth = queue.popleft()
        if folder in seen:
            continue
        seen.add(folder)
        result["directories"] += 1
        result["max_depth"] = max(result["max_depth"], depth)
        if (folder / ".git").exists():
            try:
                private = Path(os.fsdecode(git(folder, "rev-parse", "--path-format=absolute", "--git-dir")[:-1])).resolve()
                if private not in identities:
                    identities.add(private)
                    result["worktrees"] += 1
                    tracked = list(dict.fromkeys(git(folder, "ls-files", "-z").split(b"\0")[:-1]))
                    untracked = git(folder, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")[:-1]
                    result["tracked"] += len(tracked)
                    result["untracked"] += len(untracked)
                    index = private / "index"
                    result["index_bytes"] += index.stat().st_size if index.exists() else 0
                    for name in tracked:
                        try:
                            stat = (folder / os.fsdecode(name)).lstat()
                            if not (folder / os.fsdecode(name)).is_dir():
                                result["tracked_bytes_on_disk"] += stat.st_size
                                result["largest_tracked_file"] = max(result["largest_tracked_file"], stat.st_size)
                        except OSError:
                            # Tracked deletions are valid, not scan failures.
                            pass
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                result["errors"] += 1
                key = type(error).__name__
                result["error_categories"][key] = result["error_categories"].get(key, 0) + 1
        try:
            with os.scandir(folder) as entries:
                for entry in entries:
                    if entry.is_symlink():
                        result["symlinks_skipped"] += 1
                    elif entry.is_dir(follow_symlinks=False):
                        if entry.name in EXCLUDED:
                            result["excluded_directories"] += 1
                        else:
                            queue.append((Path(entry.path), depth + 1))
        except OSError as error:
            result["errors"] += 1
            key = type(error).__name__
            result["error_categories"][key] = result["error_categories"].get(key, 0) + 1
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("roots", type=Path, nargs="+")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    started = time.monotonic()
    identities = set()
    results = {f"workspace-{i+1}": profile(root, identities) for i, root in enumerate(args.roots)}
    report = {"version": 2, "kind": "R", "source_contents_retained": False,
              "source_contents_uploaded": False, "git_status_used": False,
              "roots_in_order": [str(p.absolute()) for p in args.roots], "workspaces": results,
              "unique_worktrees": len(identities), "duration_seconds": round(time.monotonic() - started, 3),
              "unmeasured": ["changed_files", "diff_bytes", "longest_line", "open_document_working_set", "image_decoded_bytes"]}
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
