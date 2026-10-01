//! Turns a batch of watched paths into the smallest refresh: which repositories need a status
//! query, which explorer directories changed, and which index entries to add or remove.
//!
//! Git-ignored paths (target/, node_modules, build output) are dropped. Ignore verdicts for
//! directories are cached per repository, so a long build that writes thousands of files
//! costs one `git check-ignore` for its output directory and nothing afterwards.

use crate::files::PathIndex;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};
use workspace_editor_core::RepoId;

/// Cached directory verdicts kept per window; cleared when it grows past this.
const MAX_CACHED_DIRS: usize = 20_000;

pub struct RepoRoots {
    pub id: RepoId,
    pub worktree: PathBuf,
    /// The private Git directory and the common directory (linked worktrees share it).
    pub git_dirs: Vec<PathBuf>,
}

#[derive(Default, Debug)]
pub struct Plan {
    /// Rediscover repositories and rebuild everything (new `.git`, watcher overflow).
    pub full: bool,
    pub rebuild_index: bool,
    pub repos: HashSet<RepoId>,
    /// Explorer directories whose listing changed.
    pub dirs: BTreeSet<PathBuf>,
    pub added: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// Relevant (not ignored) paths whose content or existence changed.
    pub files: BTreeSet<PathBuf>,
    pub considered: usize,
    pub ignored: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        !self.full
            && !self.rebuild_index
            && self.repos.is_empty()
            && self.dirs.is_empty()
            && self.files.is_empty()
    }

    pub fn merge(&mut self, next: Plan) {
        self.full |= next.full;
        self.rebuild_index |= next.rebuild_index;
        self.repos.extend(next.repos);
        self.dirs.extend(next.dirs);
        self.added.extend(next.added);
        self.removed.extend(next.removed);
        self.files.extend(next.files);
        self.considered += next.considered;
        self.ignored += next.ignored;
    }
}

#[derive(Default)]
pub struct IgnoreCache {
    dirs: HashMap<PathBuf, bool>,
}

impl IgnoreCache {
    fn forget(&mut self, under: &Path) {
        self.dirs.retain(|dir, _| !dir.starts_with(under));
    }
}

/// `check(repo, relative paths)` returns the ignored subset (one Git process per call).
pub type CheckIgnore<'a> = dyn Fn(&RepoRoots, &[PathBuf]) -> io::Result<Vec<PathBuf>> + 'a;

struct Pending<'a> {
    repo: &'a RepoRoots,
    path: PathBuf,
}

pub fn plan(
    paths: &[PathBuf],
    repos: &[RepoRoots],
    index: Option<&PathIndex>,
    cache: &mut IgnoreCache,
    check: &CheckIgnore<'_>,
) -> Plan {
    let mut plan = Plan::default();
    if cache.dirs.len() > MAX_CACHED_DIRS {
        cache.dirs.clear();
    }
    let mut pending: Vec<Pending> = Vec::new();
    let unique: BTreeSet<&PathBuf> = paths.iter().collect();
    for path in unique {
        plan.considered += 1;
        // Git metadata: refs, index and HEAD change status; object writes do not.
        let mut metadata = false;
        let mut touched = false;
        for repo in repos {
            if let Some(relative) = repo
                .git_dirs
                .iter()
                .find_map(|dir| path.strip_prefix(dir).ok())
            {
                metadata = true;
                let first = relative.components().next().map(|c| c.as_os_str());
                if first.is_some_and(|c| c == "objects" || c == "logs") {
                    continue;
                }
                if relative == Path::new("info/exclude") {
                    cache.forget(&repo.worktree);
                }
                plan.repos.insert(repo.id.clone());
                touched = true;
            }
        }
        if metadata {
            if !touched {
                plan.ignored += 1;
            }
            continue;
        }
        let name = path.file_name().unwrap_or_default();
        if name == ".git" {
            plan.full = true;
            continue;
        }
        let repo = repos
            .iter()
            .filter(|repo| path.starts_with(&repo.worktree) && *path != repo.worktree)
            .max_by_key(|repo| repo.worktree.as_os_str().len());
        if name == ".gitignore"
            && let Some(repo) = repo
        {
            cache.forget(path.parent().unwrap_or(&repo.worktree));
        }
        let indexed = index.is_some_and(|index| index.covers(path));
        match repo {
            Some(repo) if !indexed => match cached_verdict(cache, repo, path) {
                Some(true) => plan.ignored += 1,
                Some(false) => relevant(&mut plan, Some(repo), path, index),
                None => pending.push(Pending {
                    repo,
                    path: path.clone(),
                }),
            },
            _ => relevant(&mut plan, repo, path, index),
        }
    }
    resolve(&mut plan, pending, index, cache, check);
    plan
}

/// `Some(true)` when a cached ancestor directory is ignored; otherwise the path's own cached
/// verdict (directories only). Files are always checked: their name can match (`*.log`).
fn cached_verdict(cache: &IgnoreCache, repo: &RepoRoots, path: &Path) -> Option<bool> {
    if ancestors(repo, path)
        .iter()
        .any(|dir| cache.dirs.get(*dir) == Some(&true))
    {
        return Some(true);
    }
    cache.dirs.get(path).copied()
}

/// Directories between the worktree (exclusive) and `path` (exclusive), outermost first.
fn ancestors<'a>(repo: &RepoRoots, path: &'a Path) -> Vec<&'a Path> {
    let mut dirs: Vec<&Path> = path
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&repo.worktree) && *dir != repo.worktree)
        .collect();
    dirs.reverse();
    dirs
}

fn resolve(
    plan: &mut Plan,
    pending: Vec<Pending>,
    index: Option<&PathIndex>,
    cache: &mut IgnoreCache,
    check: &CheckIgnore<'_>,
) {
    // One batch per repository: unknown ancestor directories (with a trailing slash) and paths.
    let mut batches: Vec<(&RepoRoots, Vec<PathBuf>, Vec<PathBuf>)> = Vec::new();
    for item in &pending {
        let at = match batches
            .iter()
            .position(|(repo, ..)| repo.id == item.repo.id)
        {
            Some(at) => at,
            None => {
                batches.push((item.repo, Vec::new(), Vec::new()));
                batches.len() - 1
            }
        };
        let (repo, dirs, files) = &mut batches[at];
        for dir in ancestors(repo, &item.path) {
            if !cache.dirs.contains_key(dir) && !dirs.iter().any(|d| d == dir) {
                dirs.push(dir.to_path_buf());
            }
        }
        files.push(item.path.clone());
    }
    let mut ignored_files: HashSet<PathBuf> = HashSet::new();
    let mut failed: HashSet<RepoId> = HashSet::new();
    for (repo, dirs, files) in &batches {
        let relative = |path: &PathBuf, dir: bool| {
            let mut text = path
                .strip_prefix(&repo.worktree)
                .unwrap_or(path)
                .as_os_str()
                .to_owned();
            if dir {
                text.push("/");
            }
            PathBuf::from(text)
        };
        let query: Vec<PathBuf> = dirs
            .iter()
            .map(|d| relative(d, true))
            .chain(files.iter().map(|f| relative(f, false)))
            .collect();
        match check(repo, &query) {
            Ok(ignored) => {
                let ignored: HashSet<PathBuf> = ignored
                    .into_iter()
                    .map(|p| {
                        let text = p.to_string_lossy().trim_end_matches('/').to_string();
                        repo.worktree.join(text)
                    })
                    .collect();
                for dir in dirs {
                    cache.dirs.insert(dir.clone(), ignored.contains(dir));
                }
                ignored_files.extend(files.iter().filter(|f| ignored.contains(*f)).cloned());
            }
            Err(error) => {
                // Without a verdict, refreshing is the safe choice.
                eprintln!("event=check_ignore_failed error={error}");
                failed.insert(repo.id.clone());
            }
        }
    }
    for item in pending {
        let ignored = !failed.contains(&item.repo.id)
            && (ignored_files.contains(&item.path)
                || ancestors(item.repo, &item.path)
                    .iter()
                    .any(|dir| cache.dirs.get(*dir) == Some(&true)));
        if ignored {
            plan.ignored += 1;
        } else {
            relevant(plan, Some(item.repo), &item.path, index);
        }
    }
}

fn relevant(plan: &mut Plan, repo: Option<&RepoRoots>, path: &Path, index: Option<&PathIndex>) {
    plan.files.insert(path.to_path_buf());
    if let Some(repo) = repo {
        plan.repos.insert(repo.id.clone());
    }
    let parent = path.parent().map(Path::to_path_buf);
    let metadata = std::fs::symlink_metadata(path).ok();
    let Some(index) = index else {
        plan.rebuild_index = true;
        plan.dirs.extend(parent);
        return;
    };
    let indexed = index.covers(path);
    match metadata {
        Some(meta) if !indexed => {
            plan.dirs.extend(parent);
            if meta.is_dir() {
                // A new directory may arrive with many files; list it once rather than per event.
                plan.rebuild_index = true;
            } else {
                plan.added.push(path.to_path_buf());
            }
        }
        None if indexed => {
            plan.dirs.extend(parent);
            plan.removed.push(path.to_path_buf());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs};

    fn repo(root: &Path) -> RepoRoots {
        RepoRoots {
            id: RepoId(root.join(".git")),
            worktree: root.to_path_buf(),
            git_dirs: vec![root.join(".git")],
        }
    }

    /// A simulated `cargo build`: thousands of writes under target/ cost one ignore check for
    /// the directory and refresh nothing; a source edit refreshes only its repository.
    #[test]
    fn build_output_is_ignored_with_one_check() {
        let root = std::env::temp_dir().join(format!("zj-plan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target/debug/deps")).unwrap();
        fs::create_dir_all(root.join("other/src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let not_git = |_: &Path, _: &std::sync::atomic::AtomicBool| Err(io::Error::other("no"));
        let index = PathIndex::build(&root, &Default::default(), &not_git);
        let repos = [repo(&root), repo(&root.join("other"))];
        let calls = Cell::new(0);
        let check = |_: &RepoRoots, paths: &[PathBuf]| {
            calls.set(calls.get() + 1);
            Ok(paths
                .iter()
                .filter(|p| p.starts_with("target") || p.ends_with("x.log"))
                .cloned()
                .collect())
        };
        let mut cache = IgnoreCache::default();
        for batch in 0..20 {
            let paths: Vec<PathBuf> = (0..500)
                .map(|n| root.join(format!("target/debug/deps/lib{batch}-{n}.rlib")))
                .chain([root.join(".git/objects/ab/cdef")])
                .collect();
            let p = plan(&paths, &repos, Some(&index), &mut cache, &check);
            assert!(p.is_empty(), "{p:?}");
            assert_eq!(p.ignored, 501);
        }
        assert_eq!(calls.get(), 1);

        let p = plan(
            &[root.join("src/main.rs"), root.join("src/x.log")],
            &repos,
            Some(&index),
            &mut cache,
            &check,
        );
        assert_eq!(p.repos, HashSet::from([repos[0].id.clone()]));
        assert_eq!(p.files.len(), 1);
        assert!(p.dirs.is_empty(), "content edits do not touch the explorer");

        fs::write(root.join("other/src/new.rs"), "").unwrap();
        let p = plan(
            &[root.join("other/src/new.rs"), root.join("other/.git/HEAD")],
            &repos,
            Some(&index),
            &mut cache,
            &check,
        );
        assert_eq!(p.repos, HashSet::from([repos[1].id.clone()]));
        assert_eq!(p.added, vec![root.join("other/src/new.rs")]);
        assert!(p.dirs.contains(&root.join("other/src")));

        fs::remove_file(root.join("src/main.rs")).unwrap();
        let p = plan(
            &[root.join("src/main.rs")],
            &repos,
            Some(&index),
            &mut cache,
            &check,
        );
        assert_eq!(p.removed, vec![root.join("src/main.rs")]);
        let p = plan(
            &[root.join("nested/.git")],
            &repos,
            Some(&index),
            &mut cache,
            &check,
        );
        assert!(p.full);
        fs::remove_dir_all(&root).unwrap();
    }
}
