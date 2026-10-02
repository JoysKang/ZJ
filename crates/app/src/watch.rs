//! One native watcher per application, shared subscriptions, and one bounded wakeup per window.
use async_channel::{Receiver, Sender};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use workspace_editor_core::{Repository, is_excluded_dir};

/// Changed paths kept per wakeup; beyond this the window falls back to a full refresh.
const MAX_PATHS: usize = 4_096;

#[derive(Default)]
pub struct Notice {
    pub changed: bool,
    pub error: Option<String>,
    /// Paths that matched this subscriber, so the window can refresh only what they touch.
    pub paths: Vec<PathBuf>,
    /// A rescan, a watcher error or too many paths: the paths are not the whole story.
    pub overflow: bool,
}

impl Notice {
    pub fn merge(&mut self, next: Self) {
        self.changed |= next.changed;
        if next.error.is_some() {
            self.error = next.error;
        }
        self.overflow |= next.overflow;
        self.add_paths(next.paths);
    }

    fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if self.overflow {
                break;
            }
            if self.paths.len() >= MAX_PATHS {
                self.overflow = true;
                self.paths.clear();
                break;
            }
            self.paths.push(path);
        }
    }
}

struct Subscriber {
    root: PathBuf,
    paths: Vec<PathBuf>,
    repositories: HashSet<PathBuf>,
    pending: Arc<Mutex<Notice>>,
    sender: Sender<()>,
    /// An event under the root was dropped by the excluded-directory filter; a repository
    /// discovered later may track that path, so its registration forces a full refresh.
    dropped: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct Subscribers {
    next_id: u64,
    entries: HashMap<u64, Subscriber>,
}

impl Subscribers {
    fn deliver(&self, result: notify::Result<Event>) {
        if matches!(&result, Ok(event) if matches!(event.kind, EventKind::Access(_))) {
            return;
        }
        for subscriber in self.entries.values() {
            let notice = match &result {
                Ok(event) => {
                    let rescan = event.need_rescan();
                    let paths: Vec<PathBuf> = event
                        .paths
                        .iter()
                        .filter(|path| {
                            let matches = subscriber.matches(path);
                            if !matches && path.starts_with(&subscriber.root) {
                                subscriber
                                    .dropped
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            matches
                        })
                        .cloned()
                        .collect();
                    if !rescan && paths.is_empty() {
                        continue;
                    }
                    let mut notice = Notice {
                        changed: true,
                        overflow: rescan,
                        ..Default::default()
                    };
                    notice.add_paths(paths);
                    notice
                }
                Err(error) => Notice {
                    changed: true,
                    error: Some(error.to_string().chars().take(512).collect()),
                    overflow: true,
                    ..Default::default()
                },
            };
            subscriber.pending.lock().unwrap().merge(notice);
            // Full means a wakeup is already pending; the merged notice is never lost.
            let _ = subscriber.sender.try_send(());
        }
    }

    fn invalidate(&self) {
        self.deliver(Ok(
            Event::new(EventKind::Any).set_flag(notify::event::Flag::Rescan)
        ));
    }
}

impl Subscriber {
    fn matches(&self, path: &Path) -> bool {
        // A worktree can legally track files inside target/node_modules. Git decides
        // its Changes; directory-name filters are only safe outside known worktrees.
        if self.repositories.iter().any(|root| path.starts_with(root)) {
            return true;
        }
        // Extra paths are Git metadata directories, including gitfiles and linked worktrees.
        if self.paths.iter().skip(1).any(|root| path.starts_with(root)) {
            return true;
        }
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        for component in relative.components() {
            if component.as_os_str() == ".git" {
                return true;
            }
            if is_excluded_dir(component.as_os_str()) {
                return false;
            }
        }
        true
    }
}

#[derive(Default)]
struct Native {
    watcher: Option<RecommendedWatcher>,
    references: HashMap<PathBuf, usize>,
}

#[derive(Default)]
struct Shared {
    subscribers: Arc<Mutex<Subscribers>>,
    native: Mutex<Native>,
}

#[derive(Clone, Default)]
pub struct WatchService(Arc<Shared>);

pub struct Subscription {
    service: WatchService,
    id: u64,
    receiver: Receiver<()>,
    pending: Arc<Mutex<Notice>>,
    /// Serializes path ownership with native registration/release across old/new scans.
    operations: Mutex<()>,
}

fn error(error: notify::Error) -> io::Error {
    io::Error::other(error.to_string())
}

impl WatchService {
    pub fn subscribe(&self, root: &Path) -> io::Result<Arc<Subscription>> {
        let root = std::fs::canonicalize(root)?;
        let (sender, receiver) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(Notice::default()));
        let id = {
            let mut subscribers = self.0.subscribers.lock().unwrap();
            subscribers.next_id += 1;
            let id = subscribers.next_id;
            subscribers.entries.insert(
                id,
                Subscriber {
                    root: root.clone(),
                    paths: vec![],
                    repositories: HashSet::new(),
                    pending: pending.clone(),
                    sender,
                    dropped: Default::default(),
                },
            );
            id
        };
        let subscription = Arc::new(Subscription {
            service: self.clone(),
            id,
            receiver,
            pending,
            operations: Mutex::new(()),
        });
        {
            let _operation = subscription.operations.lock().unwrap();
            subscription.add_path(&root)?;
        }
        Ok(subscription)
    }
}

impl Subscription {
    fn add_path(&self, path: &Path) -> io::Result<()> {
        {
            let mut subscribers = self.service.0.subscribers.lock().unwrap();
            let subscriber = subscribers.entries.get_mut(&self.id).unwrap();
            if subscriber.paths.iter().any(|root| path.starts_with(root)) {
                return Ok(());
            }
            // Register the recipient before starting the native stream.
            subscriber.paths.push(path.to_path_buf());
        }
        let result: io::Result<bool> = (|| {
            let mut native = self.service.0.native.lock().unwrap();
            if let Some(count) = native.references.get_mut(path) {
                *count += 1;
                return Ok(false);
            }
            if native.watcher.is_none() {
                let subscribers = self.service.0.subscribers.clone();
                native.watcher = Some(
                    notify::recommended_watcher(move |event| {
                        subscribers.lock().unwrap().deliver(event);
                    })
                    .map_err(error)?,
                );
            }
            if let Err(watch_error) = native
                .watcher
                .as_mut()
                .unwrap()
                .watch(path, RecursiveMode::Recursive)
            {
                // Some backends add a path before a stream-start error; roll it back as well.
                let _ = native.watcher.as_mut().unwrap().unwatch(path);
                return Err(error(watch_error));
            }
            native.references.insert(path.to_path_buf(), 1);
            Ok(true)
        })();
        if result.is_err() {
            self.service
                .0
                .subscribers
                .lock()
                .unwrap()
                .entries
                .get_mut(&self.id)
                .unwrap()
                .paths
                .retain(|registered| registered != path);
        }
        // FSEvents reconfigures its stream on watch/unwatch; close that gap with a recheck.
        // inotify adds watches without restarting anything, so other windows keep their state.
        if !matches!(result, Ok(false)) && cfg!(target_os = "macos") {
            self.service.0.subscribers.lock().unwrap().invalidate();
        }
        result.map(|_| ())
    }

    pub fn add_repository(&self, repo: &Repository) -> io::Result<()> {
        let _operation = self.operations.lock().unwrap();
        let added = {
            let mut subscribers = self.service.0.subscribers.lock().unwrap();
            subscribers
                .entries
                .get_mut(&self.id)
                .unwrap()
                .repositories
                .insert(repo.worktree.clone())
        };
        self.add_path(&repo.common_dir)?;
        self.add_path(&repo.id.0)?;
        if added {
            // Indexing may have started before discovery broadened the event filter. Only an
            // event the filter actually dropped can be missing, so a quiet startup does not
            // pay for a second full refresh.
            let subscribers = self.service.0.subscribers.lock().unwrap();
            let subscriber = subscribers.entries.get(&self.id).unwrap();
            if !subscriber
                .dropped
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Ok(());
            }
            let mut pending = subscriber.pending.lock().unwrap();
            pending.changed = true;
            pending.overflow = true;
            drop(pending);
            let _ = subscriber.sender.try_send(());
        }
        Ok(())
    }

    pub fn retain_repositories(&self, repos: &[Repository]) {
        let _operation = self.operations.lock().unwrap();
        let removed = {
            let mut subscribers = self.service.0.subscribers.lock().unwrap();
            let subscriber = subscribers.entries.get_mut(&self.id).unwrap();
            subscriber
                .repositories
                .retain(|root| repos.iter().any(|repo| repo.worktree == *root));
            let mut removed = Vec::new();
            subscriber.paths.retain(|path| {
                let keep = *path == subscriber.root
                    || repos.iter().any(|repo| {
                        repo.common_dir.starts_with(path) || repo.id.0.starts_with(path)
                    });
                if !keep {
                    removed.push(path.clone())
                }
                keep
            });
            removed
        };
        if !removed.is_empty() {
            self.service.0.release(removed)
        }
    }

    pub async fn receive(&self) -> Notice {
        let _ = self.receiver.recv().await;
        self.take_pending()
    }

    pub fn take_pending(&self) -> Notice {
        let _ = self.receiver.try_recv();
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let subscriber = self
            .service
            .0
            .subscribers
            .lock()
            .unwrap()
            .entries
            .remove(&self.id);
        let Some(subscriber) = subscriber else { return };
        self.service.0.release(subscriber.paths);
    }
}

impl Shared {
    fn release(&self, paths: Vec<PathBuf>) {
        // Never hold the subscriber lock while FSEvents stops and joins its callback thread.
        let mut native = self.native.lock().unwrap();
        let mut failure = None;
        for path in paths {
            let Some(count) = native.references.get_mut(&path) else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                native.references.remove(&path);
                if let Some(watcher) = &mut native.watcher
                    && let Err(error) = watcher.unwatch(&path)
                {
                    eprintln!("event=watch_release_failed error={error}");
                    failure = Some(error);
                }
            }
        }
        if native.references.is_empty() {
            native.watcher = None
        }
        drop(native);
        let subscribers = self.subscribers.lock().unwrap();
        // Only FSEvents restarts its stream on unwatch (see `add_path`).
        if cfg!(target_os = "macos") {
            subscribers.invalidate();
        }
        if let Some(error) = failure {
            subscribers.deliver(Err(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs, thread,
        time::{Duration, Instant},
    };
    use workspace_editor_core::RepoId;

    #[test]
    fn bursts_errors_and_rescans_use_one_bounded_wakeup() {
        let (sender, receiver) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(Notice::default()));
        let mut subscribers = Subscribers::default();
        subscribers.entries.insert(
            1,
            Subscriber {
                root: PathBuf::from("/workspace"),
                paths: vec![PathBuf::from("/workspace"), PathBuf::from("/git/metadata")],
                repositories: HashSet::new(),
                sender,
                pending: pending.clone(),
                dropped: Default::default(),
            },
        );
        for path in [
            "/elsewhere/file",
            "/workspace/target/output",
            "/workspace/node_modules/file",
        ] {
            subscribers.deliver(Ok(Event::new(EventKind::Any).add_path(path.into())));
        }
        assert!(receiver.is_empty());
        subscribers
            .entries
            .get_mut(&1)
            .unwrap()
            .repositories
            .insert(PathBuf::from("/workspace"));
        subscribers.deliver(Ok(
            Event::new(EventKind::Any).add_path(PathBuf::from("/workspace/target/config.rs"))
        ));
        assert_eq!(receiver.len(), 1);
        for _ in 0..10_000 {
            subscribers.deliver(Ok(Event::new(EventKind::Any)
                .add_path(PathBuf::from("/workspace/.git/refs/heads/target"))));
        }
        subscribers.deliver(Err(notify::Error::generic("watch failed")));
        subscribers.invalidate();
        assert_eq!(receiver.len(), 1);
        let notice = std::mem::take(&mut *pending.lock().unwrap());
        assert!(notice.changed);
        assert!(notice.error.unwrap().contains("watch failed"));
        receiver.try_recv().unwrap();
        subscribers
            .deliver(Ok(Event::new(EventKind::Any)
                .add_path(PathBuf::from("/git/metadata/refs/heads/target"))));
        assert_eq!(receiver.len(), 1);
    }

    #[test]
    fn native_events_share_paths_and_release_external_git_references() {
        let temp = std::env::temp_dir().join(format!("zj-watch-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        let temp = fs::canonicalize(temp).unwrap();
        let root = temp.join("workspace");
        let metadata = temp.join("external-git");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&metadata).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let service = WatchService::default();
        let first = service.subscribe(&root).unwrap();
        let second = service.subscribe(&root).unwrap();
        assert_eq!(service.0.native.lock().unwrap().references[&root], 2);
        let repo = Repository {
            id: RepoId(metadata.clone()),
            common_dir: metadata.clone(),
            worktree: root.clone(),
        };
        first.add_repository(&repo).unwrap();
        first.add_repository(&repo).unwrap();
        assert_eq!(service.0.native.lock().unwrap().references[&metadata], 1);
        // Let stream reconfiguration settle, then consume its deliberately queued recheck.
        thread::sleep(Duration::from_millis(300));
        first.take_pending();
        second.take_pending();
        fs::write(root.join("src/new.txt"), "native event\n").unwrap();
        let wait = |subscription: &Subscription| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if subscription.take_pending().changed {
                    break;
                }
                assert!(Instant::now() < deadline, "native event deadline exceeded");
                thread::sleep(Duration::from_millis(20));
            }
        };
        wait(&first);
        wait(&second);
        fs::write(root.join("src/replacement"), "atomic replacement\n").unwrap();
        fs::rename(root.join("src/replacement"), root.join("src/new.txt")).unwrap();
        wait(&first);
        first.take_pending();
        fs::write(metadata.join("index"), "external index update").unwrap();
        wait(&first);
        first.retain_repositories(&[]);
        assert!(
            !service
                .0
                .native
                .lock()
                .unwrap()
                .references
                .contains_key(&metadata)
        );
        thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..20 {
                    first.add_repository(&repo).unwrap()
                }
            });
            scope.spawn(|| {
                for _ in 0..20 {
                    first.retain_repositories(&[])
                }
            });
        });
        first.retain_repositories(&[]);
        assert_eq!(service.0.native.lock().unwrap().references.len(), 1);
        drop(first);
        assert_eq!(service.0.native.lock().unwrap().references[&root], 1);
        drop(second);
        assert!(service.0.native.lock().unwrap().watcher.is_none());
        assert!(service.0.subscribers.lock().unwrap().entries.is_empty());
        assert!(service.subscribe(&temp.join("missing")).is_err());
        assert!(service.0.subscribers.lock().unwrap().entries.is_empty());
        fs::remove_dir_all(temp).unwrap();
    }
}
