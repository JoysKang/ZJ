//! Local-only revision comparisons, file history and repository-wide commit search.
use crate::{
    GitService, GraphCommit, GraphScope, Repository, error, graph, parse_graph, parse_name_status,
    validate_relative_path,
};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComparisonMode {
    #[default]
    Direct,
    MergeBase,
}

/// Resolved immutable endpoints. A later branch movement cannot change the selected comparison.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub base: String,
    pub target: String,
    pub mode: ComparisonMode,
    pub files: Vec<crate::CommitFile>,
}

#[derive(Clone, Debug)]
pub struct FileHistoryEntry {
    pub commit: GraphCommit,
    /// Path in this commit (or the deleted path for a deletion).
    pub path: PathBuf,
    pub deleted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitSearch {
    pub message: String,
    pub author: String,
    pub since: String,
    pub until: String,
    pub hash: String,
    pub path: Option<PathBuf>,
}

fn local_args(command: &str) -> Vec<OsString> {
    vec![
        "--no-lazy-fetch".into(),
        "--no-replace-objects".into(),
        "--literal-pathspecs".into(),
        command.into(),
    ]
}

fn bounded_page(skip: usize, limit: usize) -> io::Result<()> {
    if limit == 0 || limit > 500 || skip > 1_000_000 {
        return Err(error("历史分页超出限制"));
    }
    Ok(())
}

fn calendar_date(value: &str, end: bool) -> io::Result<String> {
    let invalid = || error("日期需要 YYYY-MM-DD（1970–2099 年）的有效日期");
    if value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
        || !value
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let year: u32 = value[..4].parse().map_err(|_| invalid())?;
    let month: usize = value[5..7].parse().map_err(|_| invalid())?;
    let day: u32 = value[8..].parse().map_err(|_| invalid())?;
    let days = [
        31,
        if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1970..=2099).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days[month - 1]
    {
        return Err(invalid());
    }
    Ok(format!(
        "{value}T{}Z",
        if end { "23:59:59" } else { "00:00:00" }
    ))
}

impl GitService {
    fn resolve_local_commit(
        &self,
        repo: &Repository,
        revision: &str,
        cancel: &AtomicBool,
    ) -> io::Result<String> {
        if revision.is_empty()
            || revision.len() > 1024
            || revision.starts_with('-')
            || revision.chars().any(char::is_control)
        {
            return Err(error("无效的版本引用"));
        }
        let mut args = local_args("rev-parse");
        args.extend([
            "--verify".into(),
            "--end-of-options".into(),
            format!("{revision}^{{commit}}").into(),
        ]);
        let bytes = self.run(&repo.worktree, &args, cancel)?;
        let oid = String::from_utf8_lossy(&bytes).trim().to_string();
        if !matches!(oid.len(), 40 | 64) || !graph::is_hash(&oid) {
            return Err(error("无法解析提交版本"));
        }
        Ok(oid)
    }

    /// Bounded raw blob bytes for text/history/image viewers. Never performs lazy fetching,
    /// text conversion, external diffs or filesystem traversal through the stored path.
    pub fn read_blob(
        &self,
        repo: &Repository,
        revision: &str,
        path: &Path,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<u8>> {
        validate_relative_path(path)?;
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let oid = self.resolve_local_commit(repo, revision, cancel)?;
        let mut object = OsString::from(format!("{oid}:"));
        object.push(path.as_os_str());
        let mut args = local_args("cat-file");
        args.extend(["blob".into(), object]);
        self.run(&repo.worktree, &args, cancel)
    }

    pub fn read_index_blob(
        &self,
        repo: &Repository,
        path: &Path,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<u8>> {
        validate_relative_path(path)?;
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut object = OsString::from(":");
        object.push(path.as_os_str());
        let mut args = local_args("cat-file");
        args.extend(["blob".into(), object]);
        self.run(&repo.worktree, &args, cancel)
    }

    pub fn compare_revisions(
        &self,
        repo: &Repository,
        base: &str,
        target: &str,
        mode: ComparisonMode,
        cancel: &AtomicBool,
    ) -> io::Result<Comparison> {
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut base = self.resolve_local_commit(repo, base, cancel)?;
        let target = self.resolve_local_commit(repo, target, cancel)?;
        if mode == ComparisonMode::MergeBase {
            let mut args = local_args("merge-base");
            args.extend(["--all".into(), base.clone().into(), target.clone().into()]);
            let bytes = self.run(&repo.worktree, &args, cancel)?;
            let bases: Vec<_> = std::str::from_utf8(&bytes)
                .map_err(|_| error("无效的共同祖先"))?
                .lines()
                .collect();
            if bases.len() != 1 || !graph::is_hash(bases[0]) {
                return Err(error("没有唯一共同祖先，请使用直接比较"));
            }
            base = bases[0].into();
        }
        let mut args = local_args("diff");
        args.extend([
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--name-status".into(),
            "-z".into(),
            "-M".into(),
            base.clone().into(),
            target.clone().into(),
            "--".into(),
        ]);
        let files = parse_name_status(&self.run(&repo.worktree, &args, cancel)?)?;
        Ok(Comparison {
            base,
            target,
            mode,
            files,
        })
    }

    pub fn comparison_patch(
        &self,
        repo: &Repository,
        comparison: &Comparison,
        file: Option<&crate::CommitFile>,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<u8>> {
        if !graph::is_hash(&comparison.base) || !graph::is_hash(&comparison.target) {
            return Err(error("无效的比较版本"));
        }
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut args = local_args("diff");
        args.extend([
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "--no-color".into(),
            "--binary".into(),
            "-M".into(),
            crate::FULL_CONTEXT.into(),
            crate::SRC_PREFIX.into(),
            crate::DST_PREFIX.into(),
            comparison.base.clone().into(),
            comparison.target.clone().into(),
            "--".into(),
        ]);
        if let Some(file) = file {
            validate_relative_path(&file.path)?;
            args.push(file.path.as_os_str().into());
            if let Some(original) = &file.original_path {
                validate_relative_path(original)?;
                args.push(original.as_os_str().into());
            }
        }
        self.run(&repo.worktree, &args, cancel)
    }

    /// Uses Git's history filters before paging, so matches beyond the visible graph are found.
    pub fn search_commits(
        &self,
        repo: &Repository,
        scope: &GraphScope,
        search: &CommitSearch,
        skip: usize,
        limit: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<GraphCommit>> {
        bounded_page(skip, limit)?;
        for value in [
            &search.message,
            &search.author,
            &search.since,
            &search.until,
            &search.hash,
        ] {
            if value.len() > 4096 || value.contains('\0') {
                return Err(error("提交查询过长或包含无效字符"));
            }
        }
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut args = local_args("log");
        args.extend([
            "-z".into(),
            "--no-color".into(),
            "--no-show-signature".into(),
            "--date-order".into(),
            "--decorate=full".into(),
            "--fixed-strings".into(),
            graph::GRAPH_FORMAT.into(),
            format!("--skip={skip}").into(),
            format!("--max-count={limit}").into(),
        ]);
        if !search.message.is_empty() {
            args.push(format!("--grep={}", search.message).into());
        }
        if !search.author.is_empty() {
            args.push(format!("--author={}", search.author).into());
        }
        if !search.since.is_empty() {
            args.push(format!("--since-as-filter={}", calendar_date(&search.since, false)?).into());
        }
        if !search.until.is_empty() {
            args.push(format!("--until={}", calendar_date(&search.until, true)?).into());
        }
        if !search.hash.is_empty() {
            if !graph::is_hash(&search.hash) {
                return Err(error("哈希查询需要至少四位十六进制字符"));
            }
            let oid = self.resolve_local_commit(repo, &search.hash, cancel)?;
            args.extend(["--no-walk".into(), oid.into()]);
        } else {
            match scope {
                GraphScope::All => {
                    args.extend(["--branches".into(), "--remotes".into(), "--tags".into()])
                }
                GraphScope::Local => args.extend(["--branches".into(), "--tags".into()]),
                GraphScope::Branch(name) => {
                    args.push(self.resolve_local_commit(repo, name, cancel)?.into())
                }
            }
            if !matches!(scope, GraphScope::Branch(_)) && self.verify(repo, "HEAD", cancel)? {
                args.push("HEAD".into());
            }
        }
        args.push("--".into());
        if let Some(path) = &search.path {
            validate_relative_path(path)?;
            args.push(path.as_os_str().into());
        }
        parse_graph(&self.run(&repo.worktree, &args, cancel)?)
    }

    /// Git tracks renames backwards before paging. Name-status records retain each historical
    /// path, including spaces, tabs and newlines; filenames never pass through lossy UTF-8.
    pub fn file_history(
        &self,
        repo: &Repository,
        path: &Path,
        skip: usize,
        limit: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<FileHistoryEntry>> {
        validate_relative_path(path)?;
        bounded_page(skip, limit)?;
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut args = local_args("log");
        // Include the earlier pages so the path at the start of a later page is reconstructed.
        args.extend([
            "--follow".into(),
            "--name-status".into(),
            "-z".into(),
            "--no-color".into(),
            "--no-show-signature".into(),
            "--decorate=full".into(),
            graph::GRAPH_FORMAT.into(),
            format!("--max-count={}", skip + limit).into(),
            "HEAD".into(),
            "--".into(),
            path.as_os_str().into(),
        ]);
        parse_history(&self.run(&repo.worktree, &args, cancel)?, path)
            .map(|entries| entries.into_iter().skip(skip).collect())
    }

    /// A single ordinary text file's target, parsed by Git rather than guessing quoted headers.
    pub fn patch_target(
        &self,
        repo: &Repository,
        patch: &str,
        cancel: &AtomicBool,
    ) -> io::Result<PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        if patch.is_empty() || patch.len() > crate::OUTPUT_LIMIT || patch.contains('\0') {
            return Err(error("补丁为空、过大或包含 NUL"));
        }
        if patch.lines().any(|line| {
            ["rename from ", "rename to ", "copy from ", "copy to "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
                || [
                    "new file mode ",
                    "deleted file mode ",
                    "old mode ",
                    "new mode ",
                ]
                .iter()
                .any(|prefix| {
                    line.strip_prefix(prefix)
                        .is_some_and(|mode| !matches!(mode, "100644" | "100755"))
                })
                || (line.starts_with("index ")
                    && matches!(line.split_whitespace().last(), Some("120000" | "160000")))
        }) {
            return Err(error(
                "应用仅支持普通文本文件，不支持重命名、复制、符号链接或子模块",
            ));
        }
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut args = local_args("apply");
        args.extend(["--numstat".into(), "-z".into(), "-".into()]);
        let output =
            self.run_with_input(&repo.worktree, &args, cancel, false, Some(patch.as_bytes()))?;
        let body = output
            .strip_suffix(b"\0")
            .ok_or_else(|| error("无法识别补丁目标"))?;
        let fields: Vec<_> = body.splitn(3, |b| *b == b'\t').collect();
        if fields.len() != 3
            || fields[2].contains(&0)
            || fields[2].is_empty()
            || !fields[..2]
                .iter()
                .all(|f| !f.is_empty() && f.iter().all(u8::is_ascii_digit))
        {
            return Err(error(
                "应用仅支持单个普通文本文件；不支持二进制、多文件或重命名补丁",
            ));
        }
        let path = PathBuf::from(OsString::from_vec(fields[2].to_vec()));
        validate_relative_path(&path)?;
        Ok(path)
    }

    /// Read-only preflight; application must still go through WriteOperation::ApplyPatch and
    /// its expected status/version check. No --unsafe-paths, external helpers or network.
    pub fn check_patch(
        &self,
        repo: &Repository,
        patch: &str,
        cached: bool,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        if patch.is_empty() || patch.len() > crate::OUTPUT_LIMIT || patch.contains('\0') {
            return Err(error("补丁为空、过大或包含 NUL"));
        }
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        let mut args = local_args("apply");
        args.extend(["--check".into(), "--whitespace=nowarn".into()]);
        if cached {
            args.push("--cached".into());
        }
        args.push("-".into());
        self.run_with_input(&repo.worktree, &args, cancel, false, Some(patch.as_bytes()))
            .map(|_| ())
    }
}

fn parse_history(bytes: &[u8], initial: &Path) -> io::Result<Vec<FileHistoryEntry>> {
    let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
    let mut entries = Vec::new();
    let mut index = 0;
    let mut path = initial.to_path_buf();
    while index < fields.len() {
        while fields
            .get(index)
            .is_some_and(|f| f.is_empty() || *f == b"\n")
        {
            index += 1;
        }
        if index == fields.len() {
            break;
        }
        if index + 7 > fields.len() {
            return Err(error("无效的文件历史"));
        }
        let mut record = Vec::new();
        for field in &fields[index..index + 7] {
            record.extend_from_slice(field);
            record.push(0);
        }
        let commit = parse_graph(&record)?
            .pop()
            .ok_or_else(|| error("无效的历史提交"))?;
        index += 7;
        let mut deleted = false;
        let mut previous = None;
        while index < fields.len() {
            let field = fields[index];
            if field.is_empty() || field == b"\n" {
                index += 1;
                continue;
            }
            if std::str::from_utf8(field).is_ok_and(graph::is_hash) {
                break;
            }
            let status = field.strip_prefix(b"\n").unwrap_or(field);
            let count = if matches!(status.first(), Some(b'R' | b'C')) {
                3
            } else {
                2
            };
            if index + count > fields.len() {
                return Err(error("无效的历史路径记录"));
            }
            let mut names = Vec::new();
            names.extend_from_slice(status);
            names.push(0);
            for field in &fields[index + 1..index + count] {
                names.extend_from_slice(field);
                names.push(0);
            }
            for file in parse_name_status(&names)? {
                if file.path == path {
                    deleted = file.status == 'D';
                    previous = file.original_path;
                }
            }
            index += count;
        }
        entries.push(FileHistoryEntry {
            commit,
            path: path.clone(),
            deleted,
        });
        if let Some(original) = previous {
            path = original;
        }
    }
    Ok(entries)
}
