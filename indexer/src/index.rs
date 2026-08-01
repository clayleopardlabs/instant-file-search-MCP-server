//! In-memory file index shared between the scanner, USN watcher, and query engine.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::mft::IndexedFile;

/// Thread-safe index of every known file.
#[derive(Default)]
pub struct FileIndex {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Full path -> entry
    entries: HashMap<String, IndexedFile>,
    /// MFT record number -> full path (USN parent resolution)
    refs: HashMap<u64, String>,
}

impl FileIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entire index (initial scan).
    pub fn replace(&self, entries: Vec<IndexedFile>) -> usize {
        let mut inner = self.inner.write().unwrap();
        let n = entries.len();
        inner.entries.clear();
        inner.refs.clear();
        for e in entries {
            inner.refs.insert(e.file_ref, e.path.clone());
            inner.entries.insert(e.path.clone(), e);
        }
        n
    }

    /// Remove all entries under a volume prefix, then insert the fresh scan.
    pub fn replace_volume(&self, prefix: &str, entries: Vec<IndexedFile>) -> usize {
        let mut inner = self.inner.write().unwrap();
        let doomed: Vec<String> = inner
            .entries
            .keys()
            .filter(|p| p.starts_with(prefix))
            .cloned()
            .collect();
        for p in doomed {
            inner.entries.remove(&p);
        }
        inner.refs.clear();
        let pairs: Vec<(u64, String)> = inner
            .entries
            .iter()
            .map(|(path, e)| (e.file_ref, path.clone()))
            .collect();
        for (r, p) in pairs {
            inner.refs.insert(r, p);
        }
        for e in entries {
            inner.refs.insert(e.file_ref, e.path.clone());
            inner.entries.insert(e.path.clone(), e);
        }
        inner.entries.len()
    }

    /// Insert or update one entry.
    pub fn upsert(&self, entry: IndexedFile) {
        let mut inner = self.inner.write().unwrap();
        let old_ref = inner.entries.get(&entry.path).map(|old| old.file_ref);
        if let Some(r) = old_ref {
            inner.refs.remove(&r);
        }
        inner.refs.insert(entry.file_ref, entry.path.clone());
        inner.entries.insert(entry.path.clone(), entry);
    }

    /// Remove an entry by full path.
    pub fn remove(&self, path: &str) {
        let mut inner = self.inner.write().unwrap();
        if let Some(old) = inner.entries.remove(path) {
            inner.refs.remove(&old.file_ref);
        }
    }

    /// Resolve an MFT record number to a full path (USN parent resolution).
    pub fn path_by_ref(&self, file_ref: u64) -> Option<String> {
        self.inner.read().unwrap().refs.get(&file_ref).cloned()
    }

    pub fn len(&self) -> usize {
        self.inner.read().unwrap().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot all entries (used by query engine).
    pub fn snapshot_map(&self) -> HashMap<String, IndexedFile> {
        self.inner.read().unwrap().entries.clone()
    }

    /// Query under the read lock: pass the entries iterator to `f`.
    pub fn with_entries<R>(&self, f: impl FnOnce(&HashMap<String, IndexedFile>) -> R) -> R {
        let inner = self.inner.read().unwrap();
        f(&inner.entries)
    }
}

/// Convenience alias for `Arc<FileIndex>`.
pub type SharedIndex = Arc<FileIndex>;
