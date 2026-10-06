//! Serialize mutations of the same file (pi's `withFileMutationQueue`).
//! Mutations of different files still run in parallel.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use tokio::sync::OwnedMutexGuard;

type Locks = Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>;

static LOCKS: LazyLock<Locks> = LazyLock::new(Default::default);

/// Wait for exclusive access to `path` (a real path).
pub async fn lock(path: &Path) -> OwnedMutexGuard<()> {
    let lock = {
        let mut locks = LOCKS.lock().unwrap();
        // Drop locks nobody holds or waits for.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        locks.entry(path.to_path_buf()).or_default().clone()
    };
    lock.lock_owned().await
}
