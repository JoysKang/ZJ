#!/usr/bin/env python3
"""Create a fresh F fixture. Never modifies or removes an existing destination."""
import argparse
import json
import os
import subprocess
from pathlib import Path


def git(root, *args, expected=0):
    result = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-C", str(root), *args],
                            capture_output=True, timeout=30,
                            env={**{key: value for key, value in os.environ.items() if not key.startswith("GIT_")}, "GIT_AUTHOR_DATE": "2026-09-30T00:00:00Z",
                                 "GIT_COMMITTER_DATE": "2026-09-30T00:00:00Z"})
    if result.returncode != expected:
        raise RuntimeError(f"fixture git {args[0]} failed: {result.stderr.decode(errors='replace')}")
    return result.stdout


def init(root, separate=None, commit=True):
    root.mkdir(parents=True)
    args = ["init", "-b", "main"]
    if separate:
        args += ["--separate-git-dir", str(separate)]
    git(root, *args)
    git(root, "config", "user.name", "Fixture")
    git(root, "config", "user.email", "fixture@example.invalid")
    git(root, "config", "commit.gpgsign", "false")
    (root / "src").mkdir()
    (root / "src/main.rs").write_text('fn main() { println!("初始文本"); }\n')
    (root / "README.md").write_text("# 示例\n\n- 中文输入\n\n| 项目 | 数量 |\n| --- | --- |\n| 文件 | 1 |\n")
    if commit:
        git(root, "add", ".")
        git(root, "commit", "-m", "fixture baseline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    root = args.destination.absolute()
    root.mkdir(parents=True, exist_ok=False)
    repos = []
    roots = [root / f"workspace-{i+1}" for i in range(3)]
    for w, workspace in enumerate(roots):
        for i in range(10):
            repo = workspace / f"repo-{i:02}"
            init(repo, separate=root / "private-git" if w == 0 and i == 9 else None, commit=i != 6)
            repos.append(repo)
            file = repo / "src/main.rs"
            if i == 0:
                file.write_text('fn main() { println!("工作树修改"); }\n')
            elif i == 1:
                file.write_text('fn main() { println!("已暂存"); }\n')
                git(repo, "add", "src/main.rs")
                file.write_text('fn main() { println!("再次修改"); }\n')
            elif i == 2:
                git(repo, "mv", "src/main.rs", "src/renamed file.rs")
            elif i == 3:
                (repo / "空格 换行\n文件.txt").write_text("未跟踪\n")
            elif i == 4:
                file.unlink()
            elif i == 5:
                git(repo, "checkout", "-b", "other")
                file.write_text("other\n")
                git(repo, "commit", "-am", "other change")
                git(repo, "checkout", "main")
                file.write_text("main\n")
                git(repo, "commit", "-am", "main change")
                git(repo, "merge", "other", expected=1)
            elif i == 7:
                (repo / "binary.bin").write_bytes(b"\0binary\xff")
                git(repo, "add", "binary.bin")
                git(repo, "commit", "-m", "binary baseline")
                (repo / "binary.bin").write_bytes(b"\0changed\xff")
        nested = workspace / "repo-00" / "nested"
        init(nested)
        repos.append(nested)
    source = roots[0] / "repo-08"
    for w in (1, 2):
        linked = roots[w] / "linked"
        git(source, "worktree", "add", "-b", f"linked-{w}", str(linked))
        (linked / "src/main.rs").write_text(f"// linked {w}\n")
        repos.append(linked)
    helper = root / "submodule-source"
    init(helper)
    parent = roots[2] / "repo-08"
    git(parent, "-c", "protocol.file.allow=always", "submodule", "add", str(helper), "modules/helper")
    git(parent, "commit", "-am", "add submodule")
    repos.append(parent / "modules/helper")
    manifest = {"version": 1, "kind": "F", "seed": "fixed-v1", "workspaces": [str(p) for p in roots],
                "independent_worktrees": len(repos), "repositories": [str(p) for p in repos]}
    (root / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({k: v for k, v in manifest.items() if k != "repositories"}, ensure_ascii=False))


if __name__ == "__main__":
    main()
