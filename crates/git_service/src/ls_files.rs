//! Parser for `git ls-files -z --stage --cached --others`.
use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListedKind {
    File,
    /// A gitlink (mode 160000): an initialized submodule is a directory with its own listing.
    Submodule,
    /// Git prints an untracked nested repository as `dir/` instead of descending into it.
    NestedRepository,
}

/// Staged records look like `<mode> <object> <stage>\t<path>`; untracked records are bare paths.
/// Conflicted paths appear once per stage and are reported once.
pub fn parse_ls_files(bytes: &[u8]) -> Vec<(PathBuf, ListedKind)> {
    let mut seen = std::collections::HashSet::new();
    let mut listed = Vec::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let (path, kind) = match staged(record) {
            Some((mode, path)) if mode == b"160000" => (path, ListedKind::Submodule),
            Some((_, path)) => (path, ListedKind::File),
            None => match record.strip_suffix(b"/") {
                Some(dir) => (dir, ListedKind::NestedRepository),
                None => (record, ListedKind::File),
            },
        };
        if path.is_empty() || !seen.insert(path) {
            continue;
        }
        listed.push((PathBuf::from(OsString::from_vec(path.to_vec())), kind));
    }
    listed
}

fn staged(record: &[u8]) -> Option<(&[u8], &[u8])> {
    let tab = record.iter().position(|b| *b == b'\t')?;
    let mut fields = record[..tab].split(|b| *b == b' ');
    let mode = fields.next()?;
    let object = fields.next()?;
    let stage = fields.next()?;
    let valid = mode.len() == 6
        && mode.iter().all(|b| (b'0'..=b'7').contains(b))
        && (object.len() == 40 || object.len() == 64)
        && object.iter().all(u8::is_ascii_hexdigit)
        && matches!(stage, b"0" | b"1" | b"2" | b"3")
        && fields.next().is_none();
    valid.then(|| (mode, &record[tab + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn staged_untracked_nested_and_submodules() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let raw = format!(
            "inner/\0new file.txt\0100644 {oid} 0\tsrc/a b.rs\0\
             160000 {oid} 0\tsub\0100644 {oid} 1\tboth\0100644 {oid} 2\tboth\0"
        );
        let mut raw = raw.into_bytes();
        raw.extend_from_slice(b"raw\xff\0");
        let listed = parse_ls_files(&raw);
        let names: Vec<_> = listed
            .iter()
            .map(|(p, k)| (p.as_os_str().as_bytes().to_vec(), *k))
            .collect();
        assert_eq!(
            names,
            vec![
                (b"inner".to_vec(), ListedKind::NestedRepository),
                (b"new file.txt".to_vec(), ListedKind::File),
                (b"src/a b.rs".to_vec(), ListedKind::File),
                (b"sub".to_vec(), ListedKind::Submodule),
                (b"both".to_vec(), ListedKind::File),
                (b"raw\xff".to_vec(), ListedKind::File),
            ]
        );
    }

    #[test]
    fn untracked_path_with_tab_is_not_misread_as_staged() {
        let listed = parse_ls_files(b"weird\tname\0");
        assert_eq!(listed[0].0, PathBuf::from("weird\tname"));
        assert_eq!(listed[0].1, ListedKind::File);
    }
}
