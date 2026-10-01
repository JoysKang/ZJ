//! Workspace symbol index for cross-file go to definition: name → definitions, built lazily
//! in the background from the quick-open path index and updated per file from watch events.
//! References are not stored; ⇧F12 scans candidate files on demand instead.

use crate::symbols::{self, Kind, Symbol};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

/// One definition, 20 bytes; the name lives in the shared `names` text.
#[derive(Clone, Copy, Debug)]
struct Def {
    name: u32,
    name_len: u16,
    len: u16,
    file: u32,
    line: u32,
    column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub len: u32,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub files: usize,
    pub definitions: usize,
    pub bytes: usize,
    pub seconds: f64,
}

/// Definitions sorted by name in one flat vector (binary search), names in one string, and
/// kinds in a parallel byte vector: a few allocations in total instead of one per symbol, so
/// a 5k-file workspace stays at a few megabytes.
#[derive(Clone, Default)]
pub struct SymbolIndex {
    files: Vec<PathBuf>,
    file_ids: HashMap<PathBuf, u32>,
    names: String,
    defs: Vec<Def>,
    kinds: Vec<Kind>,
    pub stats: Stats,
}

/// The navigation language of a file, if it has queries.
pub fn language(path: &Path) -> Option<&'static str> {
    let language = crate::languages::for_path(path).0;
    symbols::supported(language).then_some(language)
}

/// Reads a file the way the editor shows it (BOM removed), within the navigation size cap.
pub fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() as usize > symbols::MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    })
}

fn scan_file(path: &Path) -> Option<(Vec<Symbol>, usize)> {
    let language = language(path)?;
    let text = read(path)?;
    let symbols = symbols::scan(language, &text)?;
    Some((symbols.definitions, text.len()))
}

impl SymbolIndex {
    /// Parses `paths` on up to four threads. Unsupported, oversized or unreadable files are
    /// skipped; cancellation returns what was parsed so far.
    pub fn build(paths: Vec<PathBuf>, cancel: &AtomicBool) -> SymbolIndex {
        let started = Instant::now();
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| language(p).is_some())
            .collect();
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<(usize, Vec<Symbol>, usize)>> = Mutex::new(Vec::new());
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .clamp(1, 4);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(index) else { break };
                        if let Some((definitions, bytes)) = scan_file(path) {
                            local.push((index, definitions, bytes));
                        }
                    }
                    results.lock().unwrap().extend(local);
                });
            }
        });
        let mut index = SymbolIndex::default();
        let mut results = results.into_inner().unwrap();
        results.sort_by_key(|(i, ..)| *i);
        for (i, definitions, bytes) in results {
            index.stats.bytes += bytes;
            index.insert(&paths[i], definitions);
        }
        index.finish();
        index.stats.seconds = started.elapsed().as_secs_f64();
        index
    }

    fn file_id(&mut self, path: &Path) -> u32 {
        if let Some(id) = self.file_ids.get(path) {
            return *id;
        }
        let id = self.files.len() as u32;
        self.files.push(path.to_path_buf());
        self.file_ids.insert(path.to_path_buf(), id);
        self.stats.files += 1;
        id
    }

    fn insert(&mut self, path: &Path, definitions: Vec<Symbol>) {
        let file = self.file_id(path);
        for symbol in definitions {
            if symbol.name.len() > u16::MAX as usize || self.names.len() > u32::MAX as usize / 2 {
                continue;
            }
            self.defs.push(Def {
                name: self.names.len() as u32,
                name_len: symbol.name.len() as u16,
                len: symbol.range.len().min(u16::MAX as usize) as u16,
                file,
                line: symbol.line,
                column: symbol.column,
            });
            self.kinds.push(symbol.kind);
            self.names.push_str(&symbol.name);
        }
    }

    fn name(&self, def: &Def) -> &str {
        &self.names[def.name as usize..def.name as usize + def.name_len as usize]
    }

    /// Sorts by name (kinds move with their definitions) and drops unused name text.
    fn finish(&mut self) {
        let mut order: Vec<usize> = (0..self.defs.len()).collect();
        order.sort_by(|a, b| self.name(&self.defs[*a]).cmp(self.name(&self.defs[*b])));
        let mut names = String::with_capacity(self.names.len());
        let mut defs = Vec::with_capacity(self.defs.len());
        let mut kinds = Vec::with_capacity(self.defs.len());
        for i in order {
            let mut def = self.defs[i];
            let name = self.name(&def);
            // Equal names are adjacent after sorting; share their text.
            let shared = defs
                .last()
                .filter(|last: &&Def| {
                    &names[last.name as usize..last.name as usize + last.name_len as usize] == name
                })
                .map(|last| last.name);
            def.name = match shared {
                Some(at) => at,
                None => {
                    let at = names.len() as u32;
                    names.push_str(name);
                    at
                }
            };
            defs.push(def);
            kinds.push(self.kinds[i]);
        }
        names.shrink_to_fit();
        self.names = names;
        self.defs = defs;
        self.kinds = kinds;
        self.stats.definitions = self.defs.len();
    }

    /// A copy with `changed` files re-parsed (removed files drop out).
    pub fn with_changes(&self, changed: &[PathBuf]) -> SymbolIndex {
        let mut next = self.clone();
        let ids: Vec<u32> = changed
            .iter()
            .filter_map(|path| next.file_ids.get(path).copied())
            .collect();
        if !ids.is_empty() {
            let keep: Vec<bool> = next.defs.iter().map(|d| !ids.contains(&d.file)).collect();
            let mut i = 0;
            next.defs.retain(|_| {
                i += 1;
                keep[i - 1]
            });
            let mut i = 0;
            next.kinds.retain(|_| {
                i += 1;
                keep[i - 1]
            });
        }
        for path in changed {
            if let Some((definitions, _)) = scan_file(path) {
                next.insert(path, definitions);
            }
        }
        next.finish();
        next
    }

    pub fn definitions(&self, name: &str) -> Vec<Location> {
        let start = self.defs.partition_point(|d| self.name(d) < name);
        self.defs[start..]
            .iter()
            .enumerate()
            .take_while(|(_, d)| self.name(d) == name)
            .map(|(i, def)| Location {
                path: self.files[def.file as usize].clone(),
                line: def.line,
                column: def.column,
                len: def.len as u32,
                kind: self.kinds[start + i],
            })
            .collect()
    }

    /// Files that might mention a name, for the on-demand reference scan.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Approximate heap size, for the resource log.
    pub fn heap_bytes(&self) -> usize {
        let paths: usize = self
            .files
            .iter()
            .map(|p| p.as_os_str().len() * 2 + 64)
            .sum();
        paths
            + self.names.capacity()
            + self.defs.capacity() * std::mem::size_of::<Def>()
            + self.kinds.capacity()
    }
}

/// Hands freed parser memory back to the OS after a bulk build (glibc and macOS keep it).
pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only returns free heap pages to the OS.
    unsafe {
        libc::malloc_trim(0);
    }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone asks every malloc zone to release free pages; goal 0 = all.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

/// Ranks candidate definitions for a reference in `from`: same file, same directory, files
/// the source seems to import (their stem appears in it), then by path distance.
pub fn rank(from: &Path, source: &str, candidates: &mut [Location]) {
    let dir = from.parent();
    let score = |location: &Location| {
        let same_file = location.path == from;
        let same_dir = location.path.parent() == dir;
        let stem = location
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let imported = stem.len() > 1 && source.contains(stem);
        let shared = location
            .path
            .components()
            .zip(from.components())
            .take_while(|(a, b)| a == b)
            .count();
        (
            !same_file,
            !same_dir,
            !imported,
            usize::MAX - shared,
            location.path.clone(),
            location.line,
        )
    };
    candidates.sort_by_cached_key(score);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cross_file_definitions_update_per_file() {
        let root = std::env::temp_dir().join(format!("zj-symbols-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/net")).unwrap();
        fs::create_dir_all(root.join("web")).unwrap();
        let lib = root.join("src/net/client.rs");
        fs::write(
            &lib,
            "pub struct Client;\npub fn connect() -> Client { Client }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/main.rs"),
            "mod net;\nfn main() { net::client::connect(); }\n",
        )
        .unwrap();
        fs::write(
            root.join("web/app.ts"),
            "export function connect(): void {}\n",
        )
        .unwrap();
        fs::write(root.join("notes.txt"), "connect").unwrap();
        let paths = vec![
            lib.clone(),
            root.join("src/main.rs"),
            root.join("web/app.ts"),
            root.join("notes.txt"),
        ];
        let index = SymbolIndex::build(paths, &AtomicBool::new(false));
        assert_eq!(index.stats.files, 3);
        let mut found = index.definitions("connect");
        assert_eq!(found.len(), 2);
        let main = root.join("src/main.rs");
        rank(&main, "net::client::connect()", &mut found);
        assert_eq!(found[0].path, lib);
        assert_eq!((found[0].line, found[0].column, found[0].len), (1, 7, 7));

        fs::write(&lib, "pub fn reconnect() {}\n").unwrap();
        let next = index.with_changes(std::slice::from_ref(&lib));
        assert_eq!(next.definitions("connect").len(), 1);
        assert_eq!(next.definitions("reconnect").len(), 1);
        assert!(next.definitions("Client").is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    /// `ZJ_INDEX_BENCH=/path cargo test --release bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_index_build() {
        let root = PathBuf::from(std::env::var("ZJ_INDEX_BENCH").unwrap());
        let mut paths = Vec::new();
        let mut pending = vec![root];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(path);
                } else {
                    paths.push(path);
                }
            }
        }
        let rss = || {
            fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find(|l| l.starts_with("VmRSS"))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        let total = paths.len();
        // Compile every language's queries first: they are process-wide and not per index.
        for language in crate::symbols::LANGUAGES {
            crate::symbols::scan(language, "x");
        }
        let before = rss();
        let index = SymbolIndex::build(paths, &AtomicBool::new(false));
        let after = rss();
        #[cfg(target_os = "linux")]
        // SAFETY: malloc_trim only returns free heap pages to the OS.
        unsafe {
            libc::malloc_trim(0);
        }
        let trimmed = rss();
        eprintln!(
            "files_seen={total} indexed={} definitions={} bytes={} seconds={:.2} heap_estimate_kb={} rss_delta_kb={} trimmed_delta_kb={}",
            index.stats.files,
            index.stats.definitions,
            index.stats.bytes,
            index.stats.seconds,
            index.heap_bytes() / 1024,
            after.saturating_sub(before),
            trimmed.saturating_sub(before)
        );
    }
}

#[cfg(test)]
mod query_cost {
    #[test]
    #[ignore]
    fn query_rss_per_language() {
        let rss = || {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find(|l| l.starts_with("VmRSS"))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        for language in crate::symbols::LANGUAGES {
            let before = rss();
            let started = std::time::Instant::now();
            crate::symbols::scan(language, "x");
            eprintln!(
                "query {language}: rss_kb={} ms={}",
                rss() - before,
                started.elapsed().as_millis()
            );
        }
    }
}
