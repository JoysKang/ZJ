//! Keeps the user's `~/.gitconfig` and the system config out of every Git these tests start,
//! the service's included: it keeps `GIT_CONFIG_GLOBAL` (the user's choice), so pointing it at an
//! empty file isolates the tests.
use std::{fs, path::PathBuf, sync::Once};

/// Call first in every test, before any thread or Git process is started.
pub fn hermetic() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir: PathBuf =
            std::env::temp_dir().join(format!("zj-test-gitconfig-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let config = dir.join("gitconfig");
        fs::write(&config, "").unwrap();
        // SAFETY: every test calls this first, so while it runs the other test threads are
        // either blocked on the Once or not started; none reads the environment or spawns a
        // process concurrently. The harness itself read its variables before starting tests.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", &config);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
    });
}
