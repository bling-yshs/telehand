//! The in-memory key store owned by the running server, persisted to
//! `keys.json` 100ms after the last change and on shutdown.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::keys::{self, Keys};

const DEBOUNCE: Duration = Duration::from_millis(100);
const RETRY: Duration = Duration::from_secs(1);

pub struct KeyStore {
    data_dir: PathBuf,
    keys: Mutex<Keys>,
    dirty: AtomicBool,
    changed: Notify,
}

impl KeyStore {
    pub fn new(data_dir: PathBuf, keys: Keys) -> Self {
        Self {
            data_dir,
            keys: Mutex::new(keys),
            dirty: AtomicBool::new(false),
            changed: Notify::new(),
        }
    }

    pub fn read<R>(&self, f: impl FnOnce(&Keys) -> R) -> R {
        f(&self.keys.lock().unwrap())
    }

    /// Change the keys and schedule a write.
    pub fn update<R>(&self, f: impl FnOnce(&mut Keys) -> R) -> R {
        let result = f(&mut self.keys.lock().unwrap());
        self.dirty.store(true, Ordering::SeqCst);
        self.changed.notify_one();
        result
    }

    /// Write the keys to disk if they changed since the last write.
    pub fn flush(&self) -> anyhow::Result<()> {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let snapshot = self.keys.lock().unwrap().clone();
        if let Err(e) = keys::save(&self.data_dir, &snapshot) {
            self.dirty.store(true, Ordering::SeqCst);
            return Err(e);
        }
        Ok(())
    }

    /// Persist changes until `shutdown`, then write any pending change.
    pub async fn run_flusher(self: Arc<Self>, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                _ = self.changed.notified() => {}
                _ = shutdown.cancelled() => break,
            }
            // Wait until no change happened for DEBOUNCE.
            loop {
                tokio::select! {
                    _ = self.changed.notified() => continue,
                    _ = tokio::time::sleep(DEBOUNCE) => break,
                    _ = shutdown.cancelled() => break,
                }
            }
            if let Err(e) = self.flush() {
                tracing::error!(error = %format!("{e:#}"), "failed to save keys; retrying");
                // Try again later even if nothing else changes.
                tokio::select! {
                    _ = tokio::time::sleep(RETRY) => self.changed.notify_one(),
                    _ = shutdown.cancelled() => {}
                }
            }
            if shutdown.is_cancelled() {
                break;
            }
        }
        if let Err(e) = self.flush() {
            tracing::error!(error = %format!("{e:#}"), "failed to save keys");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(dir: &std::path::Path) -> Keys {
        keys::load(dir).unwrap()
    }

    #[tokio::test]
    async fn changes_are_written_after_a_quiet_period() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(KeyStore::new(dir.path().to_path_buf(), Keys::new()));
        let shutdown = CancellationToken::new();
        let flusher = tokio::spawn(store.clone().run_flusher(shutdown.clone()));

        // Keep changing faster than the debounce: nothing is written yet.
        for i in 0..5 {
            store.update(|keys| {
                let (key, entry) = keys::new_entry(Some(format!("k{i}")));
                keys.insert(key, entry);
            });
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        assert!(stored(dir.path()).is_empty());

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(stored(dir.path()).len(), 5);

        shutdown.cancel();
        flusher.await.unwrap();
    }

    #[tokio::test]
    async fn pending_changes_are_written_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(KeyStore::new(dir.path().to_path_buf(), Keys::new()));
        let shutdown = CancellationToken::new();
        let flusher = tokio::spawn(store.clone().run_flusher(shutdown.clone()));

        store.update(|keys| {
            let (key, entry) = keys::new_entry(None);
            keys.insert(key, entry);
        });
        shutdown.cancel();
        flusher.await.unwrap();
        assert_eq!(stored(dir.path()).len(), 1);
    }

    #[tokio::test]
    async fn failed_writes_are_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        // A file where the data directory should be makes every write fail.
        std::fs::write(&dir, "").unwrap();
        let store = Arc::new(KeyStore::new(dir.clone(), Keys::new()));
        let shutdown = CancellationToken::new();
        let flusher = tokio::spawn(store.clone().run_flusher(shutdown.clone()));

        store.update(|keys| {
            let (key, entry) = keys::new_entry(None);
            keys.insert(key, entry);
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::remove_file(&dir).unwrap();

        // No further change: the retry alone must write the keys.
        tokio::time::sleep(RETRY + Duration::from_millis(500)).await;
        assert_eq!(stored(&dir).len(), 1);

        shutdown.cancel();
        flusher.await.unwrap();
    }
}
