//! Identities preserve filesystem semantics; display labels are never command arguments.

use std::os::unix::fs::MetadataExt;
use std::{fs, io, path::PathBuf};

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
}
