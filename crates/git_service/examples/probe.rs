use std::{
    path::PathBuf,
    sync::{Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};
use workspace_editor_git::{Discovery, GitService};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let roots: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        return Err("usage: probe ROOT ...".into());
    }
    let cancel = AtomicBool::new(false);
    let discover = GitService::new(2, Duration::from_secs(30))?;
    let mut repos = Vec::new();
    let mut issues = 0;
    let started = Instant::now();
    discover.discover(&roots, &cancel, |e| match e {
        Discovery::Repository(repo) => repos.push(repo),
        Discovery::Issue(_, _) => issues += 1,
        _ => {}
    });
    println!(
        "discovery worktrees={} issues={} seconds={:.3}",
        repos.len(),
        issues,
        started.elapsed().as_secs_f64()
    );
    for limit in [1, 2, 4] {
        let service = GitService::new(limit, Duration::from_secs(30))?;
        let next = Mutex::new(repos.iter());
        let results = Mutex::new((0, 0));
        let started = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..limit {
                let service = &service;
                let next = &next;
                let results = &results;
                let cancel = &cancel;
                scope.spawn(move || {
                    loop {
                        let Some(repo) = next.lock().unwrap().next() else {
                            break;
                        };
                        let result = service.status(repo, 1, cancel);
                        let mut totals = results.lock().unwrap();
                        match result {
                            Ok(s) => totals.0 += s.changes.len(),
                            Err(_) => totals.1 += 1,
                        }
                    }
                });
            }
        });
        let totals = results.into_inner().unwrap();
        println!(
            "concurrency={limit} changes={} errors={} seconds={:.3}",
            totals.0,
            totals.1,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
