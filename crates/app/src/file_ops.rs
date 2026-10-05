//! Explorer file operations: create, rename, copy, move and move to the Trash; and the
//! Search view's replace, which rewrites files atomically and only if they are unchanged.
//!
//! Every function takes absolute paths chosen in the Explorer and works on the filesystem
//! directly; names typed by the user are validated as a single path component first. Symlinks
//! are copied as links, never followed.

use crate::files::FileStamp;
use crate::replace::{self, Finder, Replacement};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    ops::Range,
    os::unix::ffi::{OsStrExt, OsStringExt},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Why a replace left a file alone.
pub const STALE: &str = "文件在搜索后已修改";

/// Replaces `path`'s contents with `bytes` through a temporary file in the same folder and a
/// rename, keeping its permissions, only if it is still the file `expected` describes.
/// Symlinks and files with several hard links are refused (a rename would detach them).
pub fn rewrite(path: &Path, expected: &FileStamp, bytes: &[u8]) -> io::Result<FileStamp> {
    let check = || -> io::Result<fs::Metadata> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("是符号链接，未替换"));
        }
        if metadata.nlink() > 1 {
            return Err(io::Error::other("有多个硬链接，未替换"));
        }
        if FileStamp::of(&metadata) != *expected {
            return Err(io::Error::other(STALE));
        }
        Ok(metadata)
    };
    let metadata = check()?;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("文件没有父目录"))?;
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".zj-replace-{}", std::process::id()));
    let temporary = dir.join(name);
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(metadata.permissions().mode()))?;
        file.sync_data()?;
        // The file may have changed while the new contents were written.
        check()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    FileStamp::read(path)
}

/// A file the Search view rewrote: enough to undo it while nobody else touched it.
pub struct Rewritten {
    pub path: PathBuf,
    pub count: usize,
    pub original: Vec<u8>,
    pub written: FileStamp,
    pub text: String,
}

/// Replaces the matches an earlier search found at `spans` in `path`, if the file is still
/// as it was then (`stamp`) and every span is still a match. A `\n` in the replacement
/// becomes the file's line ending; a byte order mark is kept.
pub fn replace_in_file(
    path: &Path,
    stamp: Option<&FileStamp>,
    finder: &Finder,
    spans: &[Range<usize>],
    replacement: &Replacement,
) -> io::Result<Rewritten> {
    let stamp = stamp.ok_or_else(|| io::Error::other(STALE))?;
    if FileStamp::read(path)? != *stamp {
        return Err(io::Error::other(STALE));
    }
    let original = fs::read(path)?;
    let text =
        std::str::from_utf8(&original).map_err(|_| io::Error::other("不是 UTF-8 文本，未替换"))?;
    let replacement = replacement.clone().with_eol(replace::eol_of(text));
    let edits = finder
        .edits_at(text, spans, &replacement)
        .ok_or_else(|| io::Error::other(STALE))?;
    let new = replace::apply(text, &edits);
    let written = rewrite(path, stamp, new.as_bytes())?;
    Ok(Rewritten {
        path: path.to_path_buf(),
        count: edits.len(),
        original,
        written,
        text: new,
    })
}

/// A name typed into the Explorer: one component, not `.` / `..`, no `/` or NUL.
pub fn validate_name(name: &str) -> io::Result<&OsStr> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(io::Error::other("名称不能为空"));
    }
    if trimmed == "." || trimmed == ".." || trimmed.contains(['/', '\0']) {
        return Err(io::Error::other("名称不能包含“/”，也不能是“.”或“..”"));
    }
    Ok(OsStr::new(trimmed))
}

pub fn new_file(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let path = dir.join(validate_name(name)?);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| exists_message(error, &path))?;
    Ok(path)
}

pub fn new_folder(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let path = dir.join(validate_name(name)?);
    fs::create_dir(&path).map_err(|error| exists_message(error, &path))?;
    Ok(path)
}

fn exists_message(error: io::Error, path: &Path) -> io::Error {
    if error.kind() == io::ErrorKind::AlreadyExists {
        io::Error::other(format!(
            "“{}”已存在",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    } else {
        error
    }
}

/// Renames within the same folder. A case-only rename on a case-insensitive volume is allowed;
/// replacing another entry is not.
pub fn rename(path: &Path, name: &str) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("无法重命名根目录"))?;
    let target = parent.join(validate_name(name)?);
    if target == path {
        return Ok(target);
    }
    if let Ok(existing) = fs::symlink_metadata(&target) {
        let source = fs::symlink_metadata(path)?;
        use std::os::unix::fs::MetadataExt;
        if (existing.dev(), existing.ino()) != (source.dev(), source.ino()) {
            return Err(exists_message(
                io::Error::from(io::ErrorKind::AlreadyExists),
                &target,
            ));
        }
    }
    fs::rename(path, &target)?;
    Ok(target)
}

/// `name`, then `name copy`, `name copy 2`, … before the extension (Finder's convention), the
/// first that does not exist in `dir`.
pub fn unique_name(dir: &Path, name: &OsStr) -> PathBuf {
    let candidate = dir.join(name);
    if fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }
    let bytes = name.as_bytes();
    // A leading dot is part of the name (`.env`), not an extension.
    let split = bytes
        .iter()
        .rposition(|b| *b == b'.')
        .filter(|i| *i > 0)
        .unwrap_or(bytes.len());
    let (stem, extension) = bytes.split_at(split);
    for n in 1.. {
        let mut next = stem.to_vec();
        next.extend_from_slice(" copy".as_bytes());
        if n > 1 {
            next.extend_from_slice(format!(" {n}").as_bytes());
        }
        next.extend_from_slice(extension);
        let candidate = dir.join(OsString::from_vec(next));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    unreachable!("an unbounded range always yields a free name")
}

/// Copies a file or folder into `dir`; a name that is taken gets a `copy` suffix.
pub fn copy_into(source: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = source
        .file_name()
        .ok_or_else(|| io::Error::other("无法复制根目录"))?;
    if dir.starts_with(source) {
        return Err(io::Error::other("不能把文件夹复制到它自己里面"));
    }
    let target = unique_name(dir, name);
    let mut skipped = 0;
    copy_recursive(source, &target, &mut skipped)?;
    if skipped > 0 {
        return Err(skipped_message(skipped, "已复制其余内容"));
    }
    Ok(target)
}

fn skipped_message(skipped: usize, rest: &str) -> io::Error {
    io::Error::other(format!(
        "跳过了 {skipped} 个无法复制的套接字或设备文件，{rest}"
    ))
}

/// Copies regular files, folders and symlinks (as links); recreates named pipes instead of
/// reading them, which would block until a writer appears. Sockets and devices cannot be
/// copied and are counted in `skipped`.
fn copy_recursive(source: &Path, target: &Path, skipped: &mut usize) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let metadata = fs::symlink_metadata(source)?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(source)?, target)
    } else if kind.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()), skipped)?;
        }
        fs::set_permissions(target, metadata.permissions())
    } else if kind.is_fifo() {
        let path =
            std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let mode = (metadata.permissions().mode() & 0o7777) as libc::mode_t;
        // SAFETY: `path` is a valid NUL-terminated string that outlives the call.
        if unsafe { libc::mkfifo(path.as_ptr(), mode) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    } else if kind.is_file() {
        fs::copy(source, target).map(|_| ())
    } else {
        *skipped += 1;
        Ok(())
    }
}

/// Moves a file or folder into `dir` (cut and paste). Moving into its own folder does nothing;
/// a name that is taken gets a `copy` suffix rather than replacing anything.
pub fn move_into(source: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = source
        .file_name()
        .ok_or_else(|| io::Error::other("无法移动根目录"))?;
    if source.parent() == Some(dir) {
        return Ok(source.to_path_buf());
    }
    if dir.starts_with(source) {
        return Err(io::Error::other("不能把文件夹移动到它自己里面"));
    }
    let target = unique_name(dir, name);
    match fs::rename(source, &target) {
        Ok(()) => Ok(target),
        // Another volume: copy, then remove the original.
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            let mut skipped = 0;
            copy_recursive(source, &target, &mut skipped)?;
            if skipped > 0 {
                return Err(skipped_message(skipped, "已复制其余内容，原位置保持不变"));
            }
            if fs::symlink_metadata(source)?.is_dir() {
                fs::remove_dir_all(source)?;
            } else {
                fs::remove_file(source)?;
            }
            Ok(target)
        }
        Err(error) => Err(error),
    }
}

/// Moves a file or folder to the Trash (macOS: Finder's Trash via NSFileManager, so it can be
/// put back; elsewhere the freedesktop.org home trash).
pub fn trash(path: &Path) -> io::Result<()> {
    fs::symlink_metadata(path)?;
    platform::trash(path)
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};

    pub fn trash(path: &Path) -> io::Result<()> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let class = |name: &std::ffi::CStr| {
            AnyClass::get(name).ok_or_else(|| io::Error::other("Foundation 不可用"))
        };
        let (strings, urls, managers) = (
            class(c"NSString")?,
            class(c"NSURL")?,
            class(c"NSFileManager")?,
        );
        // SAFETY: plain Foundation class and instance messages with the argument types their
        // signatures declare: `path` is a live NUL-terminated string for the whole block, the
        // returned objects are autoreleased and only used inside it, and `error` is a valid
        // out pointer. Null results are checked before use.
        unsafe {
            let string: *mut AnyObject = msg_send![strings, stringWithUTF8String: path.as_ptr()];
            if string.is_null() {
                return Err(io::Error::other("路径不是有效的 UTF-8"));
            }
            let url: *mut AnyObject = msg_send![urls, fileURLWithPath: string];
            let manager: *mut AnyObject = msg_send![managers, defaultManager];
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let resulting: *mut *mut AnyObject = std::ptr::null_mut();
            let ok: Bool = msg_send![
                manager,
                trashItemAtURL: url,
                resultingItemURL: resulting,
                error: &mut error
            ];
            if !ok.as_bool() {
                let mut message = "移到废纸篓失败".to_string();
                if !error.is_null() {
                    let description: *mut AnyObject = msg_send![error, localizedDescription];
                    if !description.is_null() {
                        let utf8: *const std::ffi::c_char = msg_send![description, UTF8String];
                        if !utf8.is_null() {
                            message = std::ffi::CStr::from_ptr(utf8)
                                .to_string_lossy()
                                .into_owned();
                        }
                    }
                }
                return Err(io::Error::other(message));
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::{
        fs, io,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    /// `$XDG_DATA_HOME/Trash` (default `~/.local/share/Trash`), per the freedesktop.org spec.
    fn trash_dir() -> io::Result<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .map(|dir| dir.join("Trash"))
            .ok_or_else(|| io::Error::other("找不到回收站目录（HOME 未设置）"))
    }

    pub fn trash(path: &Path) -> io::Result<()> {
        let trash = trash_dir()?;
        let (files, info) = (trash.join("files"), trash.join("info"));
        fs::create_dir_all(&files)?;
        fs::create_dir_all(&info)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("无法删除根目录"))?;
        let target = super::unique_name(&files, name);
        let stem = target
            .file_name()
            .unwrap_or(name)
            .to_string_lossy()
            .into_owned();
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let encoded: String = path
            .to_string_lossy()
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        fs::write(
            info.join(format!("{stem}.trashinfo")),
            format!("[Trash Info]\nPath={encoded}\nDeletionDate=@{seconds}\n"),
        )?;
        fs::rename(path, &target).map_err(|error| {
            let _ = fs::remove_file(info.join(format!("{stem}.trashinfo")));
            if error.raw_os_error() == Some(libc::EXDEV) {
                io::Error::other("文件与回收站不在同一磁盘，未删除")
            } else {
                error
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_in_files_keeps_line_endings_and_refuses_stale_files() {
        let root = std::env::temp_dir().join(format!("zj-replace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("中文.txt");
        fs::write(&path, "\u{feff}one foo\r\nfoo two\r\n").unwrap();
        let finder = Finder::new(&replace::Query {
            pattern: "foo".into(),
            ..Default::default()
        })
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let spans = finder.find_all(&text, 99);
        let stamp = FileStamp::read(&path).unwrap();
        // Only the second match, with a line break in the replacement.
        let regex = Finder::new(&replace::Query {
            pattern: "foo".into(),
            regex: true,
            ..Default::default()
        })
        .unwrap();
        let done = replace_in_file(
            &path,
            Some(&stamp),
            &regex,
            &spans[1..],
            &Replacement::new(r"bar\nbaz", true, false),
        )
        .unwrap();
        assert_eq!(done.count, 1);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\u{feff}one foo\r\nbar\r\nbaz two\r\n"
        );
        // The old stamp no longer matches: the file is skipped and left alone.
        let error = replace_in_file(
            &path,
            Some(&stamp),
            &finder,
            &spans,
            &Replacement::new("x", false, false),
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), STALE);
        // Undo while nobody touched it.
        rewrite(&path, &done.written, &done.original).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        // Symlinks and changed files are refused.
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let now = FileStamp::read(&path).unwrap();
        assert!(rewrite(&link, &now, b"x").is_err());
        fs::write(&path, "changed").unwrap();
        assert_eq!(rewrite(&path, &now, b"x").unwrap_err().to_string(), STALE);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn create_rename_copy_move_and_trash_in_a_temporary_folder() {
        let root = std::env::temp_dir().join(format!("zj-ops-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("dest")).unwrap();
        // Names.
        assert!(validate_name(" ").is_err() && validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
        // Create.
        let file = new_file(&root, "notes.md").unwrap();
        assert!(new_file(&root, "notes.md").is_err());
        let folder = new_folder(&root, "pkg").unwrap();
        fs::write(folder.join("lib.rs"), "fn x() {}").unwrap();
        std::os::unix::fs::symlink("lib.rs", folder.join("link.rs")).unwrap();
        // Rename.
        let renamed = rename(&file, "readme.md").unwrap();
        assert!(renamed.is_file() && !file.exists());
        fs::write(root.join("other.md"), "").unwrap();
        assert!(rename(&renamed, "other.md").is_err());
        // Copy: conflicts get Finder-style names; folders copy recursively, links stay links.
        assert_eq!(
            copy_into(&renamed, &root).unwrap(),
            root.join("readme copy.md")
        );
        assert_eq!(
            copy_into(&renamed, &root).unwrap(),
            root.join("readme copy 2.md")
        );
        fs::write(root.join(".env"), "").unwrap();
        assert_eq!(
            copy_into(&root.join(".env"), &root).unwrap(),
            root.join(".env copy")
        );
        let copied = copy_into(&folder, &root.join("dest")).unwrap();
        assert_eq!(
            fs::read_to_string(copied.join("lib.rs")).unwrap(),
            "fn x() {}"
        );
        assert!(
            fs::symlink_metadata(copied.join("link.rs"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(copy_into(&folder, &folder).is_err());
        // Move.
        let moved = move_into(&renamed, &root.join("dest")).unwrap();
        assert_eq!(moved, root.join("dest/readme.md"));
        assert!(!renamed.exists());
        assert_eq!(move_into(&moved, &root.join("dest")).unwrap(), moved);
        assert!(move_into(&root.join("dest"), &root.join("dest/pkg")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copying_a_folder_recreates_pipes_and_reports_sockets() {
        use std::os::unix::fs::FileTypeExt;
        let root = std::env::temp_dir().join(format!("zj-ops-special-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("pkg")).unwrap();
        fs::create_dir_all(root.join("dest")).unwrap();
        fs::write(root.join("pkg/lib.rs"), "fn x() {}").unwrap();
        let fifo = std::ffi::CString::new(root.join("pkg/pipe").to_str().unwrap()).unwrap();
        // SAFETY: `fifo` is a valid NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o640) }, 0);
        let (sender, receiver) = std::sync::mpsc::channel();
        let (folder, dest) = (root.join("pkg"), root.join("dest"));
        std::thread::spawn(move || {
            let _ = sender.send(copy_into(&folder, &dest));
        });
        let copied = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("copying a pipe blocked")
            .unwrap();
        let pipe = fs::symlink_metadata(copied.join("pipe")).unwrap();
        assert!(pipe.file_type().is_fifo());
        assert_eq!(pipe.permissions().mode() & 0o777, 0o640);
        assert_eq!(
            fs::read_to_string(copied.join("lib.rs")).unwrap(),
            "fn x() {}"
        );
        // A socket cannot be copied: the rest is, and the skip is reported.
        let socket = std::os::unix::net::UnixListener::bind(root.join("pkg/sock")).unwrap();
        let error = copy_into(&root.join("pkg"), &root.join("dest")).unwrap_err();
        assert!(error.to_string().contains("1 个"), "{error}");
        assert!(root.join("dest/pkg copy/lib.rs").is_file());
        drop(socket);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn trash_follows_the_freedesktop_layout() {
        let root = std::env::temp_dir().join(format!("zj-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).unwrap();
        let file = root.join("work/a b.txt");
        fs::write(&file, "x").unwrap();
        // SAFETY: the test process sets this before any other thread reads the environment
        // in this test; other tests do not read XDG_DATA_HOME.
        unsafe { std::env::set_var("XDG_DATA_HOME", root.join("data")) };
        trash(&file).unwrap();
        assert!(!file.exists());
        let trashed = root.join("data/Trash/files/a b.txt");
        assert_eq!(fs::read_to_string(&trashed).unwrap(), "x");
        let info = fs::read_to_string(root.join("data/Trash/info/a b.txt.trashinfo")).unwrap();
        assert!(
            info.contains("Path=") && info.contains("a%20b.txt"),
            "{info}"
        );
        assert!(trash(&root.join("work/missing")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
