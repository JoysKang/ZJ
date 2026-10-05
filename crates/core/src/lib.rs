//! Identities preserve filesystem semantics; display labels are never command arguments.

use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::{fs, io, path::PathBuf};

/// Generated, cache or dependency directories that repository discovery, the file index and
/// the non-Git file walk skip. Git-tracked listings additionally follow the repository's own
/// ignore rules (`dist/`, `build/` and the like are skipped only when ignored).
pub const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".uv-cache",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".gradle",
    "Pods",
    ".cache",
];

/// Whether a directory entry name is one of [`EXCLUDED_DIRS`].
pub fn is_excluded_dir(name: &std::ffi::OsStr) -> bool {
    EXCLUDED_DIRS.iter().any(|excluded| name == *excluded)
}

/// Why a directory's `.git` entry does not make it a repository.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitMarker {
    /// No `.git` entry at all.
    Missing,
    /// A `.git` directory with `HEAD`, or a `gitdir: <path>` file pointing at a directory.
    Valid,
    /// A `.git` entry that Git itself would reject (tool caches leave such files behind).
    Invalid,
}

/// Checks `dir/.git` the way Git does before running `git` there, without spawning a process.
pub fn git_marker(dir: &std::path::Path) -> GitMarker {
    let marker = dir.join(".git");
    let Ok(mut metadata) = fs::symlink_metadata(&marker) else {
        return GitMarker::Missing;
    };
    // Git stats `.git` through a symlink: a link to a Git directory (or to a gitfile) counts.
    if metadata.is_symlink() {
        match fs::metadata(&marker) {
            Ok(target) => metadata = target,
            Err(_) => return GitMarker::Invalid,
        }
    }
    if metadata.is_dir() {
        return if marker.join("HEAD").is_file() {
            GitMarker::Valid
        } else {
            GitMarker::Invalid
        };
    }
    if !metadata.is_file() || metadata.len() > 4096 {
        return GitMarker::Invalid;
    }
    let Ok(text) = fs::read(&marker) else {
        return GitMarker::Invalid;
    };
    let Some(target) = text
        .strip_prefix(b"gitdir: ")
        .map(|rest| rest.trim_ascii_end())
        .filter(|rest| !rest.is_empty())
    else {
        return GitMarker::Invalid;
    };
    let target = std::path::Path::new(std::ffi::OsStr::from_bytes(target));
    if dir.join(target).is_dir() {
        GitMarker::Valid
    } else {
        GitMarker::Invalid
    }
}

/// A linked worktree has its own private Git directory and therefore its own RepoId.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepoId(pub PathBuf);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    pub id: RepoId,
    pub worktree: PathBuf,
    pub common_dir: PathBuf,
}

/// Symlinks and hard links to the same inode resolve to the same document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug)]
pub struct DocumentIdentity {
    pub id: DocumentId,
    pub target: PathBuf,
    pub multiple_links: bool,
}

impl DocumentIdentity {
    pub fn resolve(path: &std::path::Path) -> io::Result<Self> {
        let target = fs::canonicalize(path)?;
        let metadata = fs::metadata(&target)?;
        if !metadata.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "不是普通文件"));
        }
        Ok(Self {
            id: DocumentId {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            target,
            multiple_links: metadata.nlink() > 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_share_identity_and_hard_links_are_restricted() {
        let root = std::env::temp_dir().join(format!("zj-identity-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let file = root.join("原文.txt");
        fs::write(&file, "中文").unwrap();
        std::os::unix::fs::symlink(&file, root.join("symbolic")).unwrap();
        fs::hard_link(&file, root.join("hard")).unwrap();
        let original = DocumentIdentity::resolve(&file).unwrap();
        assert_eq!(
            original.id,
            DocumentIdentity::resolve(&root.join("symbolic"))
                .unwrap()
                .id
        );
        assert_eq!(
            original.id,
            DocumentIdentity::resolve(&root.join("hard")).unwrap().id
        );
        assert!(original.multiple_links);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_markers_follow_git_rules() {
        let root = std::env::temp_dir().join(format!("zj-marker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "none",
            "repo/.git",
            "empty/.git",
            "bad",
            "linked",
            "dangling",
            "gitdirs/wt",
            "symlinked",
            "symlinked-file",
            "broken-link",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("repo/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        // uv's cache leaves `.git` files like this behind.
        fs::write(root.join("bad/.git"), "not a gitfile\n").unwrap();
        fs::write(root.join("linked/.git"), "gitdir: ../gitdirs/wt\n").unwrap();
        fs::write(root.join("dangling/.git"), "gitdir: /nonexistent/zj\n").unwrap();
        assert_eq!(git_marker(&root.join("none")), GitMarker::Missing);
        assert_eq!(git_marker(&root.join("repo")), GitMarker::Valid);
        assert_eq!(git_marker(&root.join("empty")), GitMarker::Invalid);
        assert_eq!(git_marker(&root.join("bad")), GitMarker::Invalid);
        assert_eq!(git_marker(&root.join("linked")), GitMarker::Valid);
        assert_eq!(git_marker(&root.join("dangling")), GitMarker::Invalid);
        // Git follows a `.git` symlink, to a Git directory or to a gitfile.
        let link = std::os::unix::fs::symlink;
        link(root.join("repo/.git"), root.join("symlinked/.git")).unwrap();
        link(root.join("linked/.git"), root.join("symlinked-file/.git")).unwrap();
        link(root.join("nonexistent"), root.join("broken-link/.git")).unwrap();
        assert_eq!(git_marker(&root.join("symlinked")), GitMarker::Valid);
        assert_eq!(git_marker(&root.join("symlinked-file")), GitMarker::Valid);
        assert_eq!(git_marker(&root.join("broken-link")), GitMarker::Invalid);
        assert!(is_excluded_dir(std::ffi::OsStr::new(".uv-cache")));
        assert!(!is_excluded_dir(std::ffi::OsStr::new("build")));
        fs::remove_dir_all(root).unwrap();
    }
}
