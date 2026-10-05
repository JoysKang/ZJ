//! Event-driven refresh orchestration; idle windows have no polling task or timer.
//!
//! Watched paths are coalesced (300 ms trailing quiet period, at most 1 s), turned into a
//! [`Plan`] off the UI thread, and applied as narrowly as possible: a status query for the
//! repositories they touch, a re-listing of changed explorer directories, and an incremental
//! update of the quick-open index. Git-ignored output (target/, node_modules) is dropped.
use super::Workbench;
use crate::{
    files::PathIndex,
    refresh_plan::{self, Plan, RepoRoots},
    watch::WatchService,
};
use gpui_kit::*;
use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

const QUIET_PERIOD: Duration = Duration::from_millis(300);
const MAX_MERGE: Duration = Duration::from_secs(1);

/// A computed plan plus the index snapshot it was computed against.
struct Computed {
    plan: Plan,
    base: Option<Arc<PathIndex>>,
    index: Option<PathIndex>,
}

impl Workbench {
    fn repo_roots(&self) -> Vec<RepoRoots> {
        self.groups
            .iter()
            .map(|g| RepoRoots {
                id: g.repo.id.clone(),
                worktree: g.repo.worktree.clone(),
                git_dirs: vec![g.repo.id.0.clone(), g.repo.common_dir.clone()],
            })
            .collect()
    }

    pub(super) fn start_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = &self.root else { return };
        let subscription = match cx.global::<WatchService>().subscribe(root) {
            Ok(subscription) => subscription,
            Err(error) => {
                self.watch.error = Some(format!("文件监听不可用：{error}；请手动刷新"));
                return;
            }
        };
        self.watch.error = subscription.take_pending().error;
        self.watch.subscription = Some(subscription.clone());
        self.watch.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let mut notice = subscription.receive().await;
                if !notice.changed && notice.error.is_none() {
                    continue;
                }
                let started = Instant::now();
                if this
                    .update(cx, |this, _| this.watch.debouncing = true)
                    .is_err()
                {
                    break;
                }
                loop {
                    cx.background_executor().timer(QUIET_PERIOD).await;
                    let next = subscription.take_pending();
                    let another_change = next.changed;
                    notice.merge(next);
                    if !another_change || started.elapsed() >= MAX_MERGE {
                        break;
                    }
                }
                let Ok((roots, base, cache, service, open)) = this.update(cx, |this, _| {
                    (
                        this.repo_roots(),
                        this.index.clone(),
                        this.watch.ignore_cache.clone(),
                        this.service.clone(),
                        this.documents
                            .iter()
                            .filter(|doc| !doc.untitled)
                            .map(|doc| doc.path.clone())
                            .collect::<BTreeSet<_>>(),
                    )
                }) else {
                    break;
                };
                let overflow = notice.overflow;
                let paths = std::mem::take(&mut notice.paths);
                // Open tabs follow their files even when Git ignores them (.env, target/…)
                // or the refresh itself waits for a build to finish.
                let open_changed: BTreeSet<_> = paths
                    .iter()
                    .filter(|p| open.contains(*p))
                    .cloned()
                    .collect();
                let computed = cx
                    .background_spawn(async move {
                        if overflow {
                            return Computed {
                                plan: Plan {
                                    full: true,
                                    ..Default::default()
                                },
                                base,
                                index: None,
                            };
                        }
                        let check = |repo: &RepoRoots, paths: &[std::path::PathBuf]| {
                            let repository = workspace_editor_core::Repository {
                                id: repo.id.clone(),
                                worktree: repo.worktree.clone(),
                                common_dir: repo.git_dirs[1].clone(),
                            };
                            service.check_ignore(&repository, paths, &AtomicBool::new(false))
                        };
                        let plan = refresh_plan::plan(
                            &paths,
                            &roots,
                            base.as_deref(),
                            &mut cache.lock().unwrap(),
                            &check,
                        );
                        let index = base
                            .as_ref()
                            .filter(|_| {
                                !plan.full
                                    && !plan.rebuild_index
                                    && (!plan.added.is_empty() || !plan.removed.is_empty())
                            })
                            .map(|index| index.with_changes(&plan.added, &plan.removed));
                        Computed { plan, base, index }
                    })
                    .await;
                eprintln!(
                    "event=watch_flush paths={} ignored={} repos={} dirs={} files={} full={}",
                    computed.plan.considered,
                    computed.plan.ignored,
                    computed.plan.repos.len(),
                    computed.plan.dirs.len(),
                    computed.plan.files.len(),
                    computed.plan.full
                );
                if this
                    .update_in(cx, |this, window, cx| {
                        if let Some(error) = notice.error {
                            this.watch.error = Some(format!("文件监听错误：{error}；请手动刷新"));
                        }
                        this.watch.debouncing = false;
                        if !open_changed.is_empty() {
                            this.check_disk(Some(&open_changed), window, cx);
                        }
                        this.queue_plan(computed, window, cx);
                        this.flush_workspace_refresh(window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn queue_plan(&mut self, computed: Computed, window: &mut Window, cx: &mut Context<Self>) {
        let Computed {
            mut plan,
            base,
            index,
        } = computed;
        if plan.full {
            self.watch.refresh_pending = true;
            self.invalidate_preview(cx);
            return;
        }
        if plan.is_empty() {
            return;
        }
        // The index swap is cheap and independent of other work; a stale base means another
        // update won the race, so list again instead of losing this change.
        if let Some(index) = index {
            let current = self.index.as_ref();
            if base.is_some() && current.is_some_and(|c| Arc::ptr_eq(c, base.as_ref().unwrap())) {
                self.index = Some(Arc::new(index));
                self.update_quick_open(window, cx);
            } else {
                plan.rebuild_index = true;
            }
        }
        match &mut self.watch.pending_plan {
            Some(pending) => pending.merge(plan),
            None => self.watch.pending_plan = Some(plan),
        }
    }

    /// The watcher already reports changes; activation only re-queries status (cheap), unless
    /// watching is unavailable.
    pub(super) fn refresh_on_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch.subscription.is_none() || self.watch.error.is_some() {
            self.watch.refresh_pending = true;
        } else {
            let repos: HashSet<_> = self.groups.iter().map(|g| g.repo.id.clone()).collect();
            let plan = Plan {
                repos,
                ..Default::default()
            };
            match &mut self.watch.pending_plan {
                Some(pending) => pending.merge(plan),
                None => self.watch.pending_plan = Some(plan),
            }
        }
        self.flush_workspace_refresh(window, cx);
    }

    pub(super) fn flush_workspace_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch.debouncing
            || self.loading
            || self.index_task.is_some()
            || !self.explorer.tasks.is_empty()
        {
            return;
        }
        if self.watch.refresh_pending {
            self.watch.refresh_pending = false;
            self.watch.pending_plan = None;
            // Re-enumeration preserves expansion and never forces open a manually collapsed path.
            let reveal_pending = self.explorer.reveal_pending;
            self.refresh_tree(window, cx);
            self.explorer.reveal_pending = reveal_pending;
            self.refresh(window, cx);
            self.check_disk(None, window, cx);
            return;
        }
        let Some(plan) = self.watch.pending_plan.take() else {
            return;
        };
        self.apply_files_changed(&plan, window, cx);
        if plan.rebuild_index
            && let Some(root) = self.root.clone()
        {
            self.build_index(root, window, cx);
        }
        for dir in &plan.dirs {
            if self.explorer.expanded.contains(dir) {
                self.reload_directory(dir.clone(), window, cx);
            }
        }
        if !plan.repos.is_empty() {
            if self
                .diff
                .tab
                .as_ref()
                .and_then(|diff| diff.request())
                .is_some_and(|request| plan.repos.contains(&request.repo.id))
            {
                self.invalidate_preview(cx);
            }
            self.refresh_repos(Some(plan.repos), window, cx);
        }
        cx.notify();
    }

    /// Hook for per-file consumers (the symbol index) of a partial refresh.
    fn apply_files_changed(&mut self, plan: &Plan, window: &mut Window, cx: &mut Context<Self>) {
        self.update_symbol_index(plan.files.iter().cloned().collect(), cx);
        // Open tabs follow their files (our own saves record their state and do not count).
        self.check_disk(Some(&plan.files), window, cx);
    }

    pub(super) fn invalidate_preview(&mut self, cx: &mut Context<Self>) {
        self.diff
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.diff.generation += 1;
        self.diff.task = None;
        if self.diff.tab.is_some() {
            self.diff.stale = true;
        }
        cx.notify();
    }
}

/// File watching: the shared subscription, the debounce task and refreshes waiting for a quiet moment.
pub(super) struct WatchState {
    pub(super) subscription: Option<Arc<crate::watch::Subscription>>,
    pub(super) task: Option<Task<()>>,
    pub(super) error: Option<String>,
    pub(super) debouncing: bool,
    /// A full refresh (rediscovery, tree and index rebuild) is pending.
    pub(super) refresh_pending: bool,
    /// Partial refresh from file watching, applied when the window is not busy.
    pub(super) pending_plan: Option<crate::refresh_plan::Plan>,
    pub(super) ignore_cache: Arc<std::sync::Mutex<crate::refresh_plan::IgnoreCache>>,
}
