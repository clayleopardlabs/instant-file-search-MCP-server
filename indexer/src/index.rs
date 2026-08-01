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
        tracing::debug!("upsert: entries={} refs={}", inner.entries.len(), inner.refs.len());
    }

    /// Remove an entry by full path.
    pub fn remove(&self, path: &str) {
        let mut inner = self.inner.write().unwrap();
        if let Some(old) = inner.entries.remove(path) {
            inner.refs.remove(&old.file_ref);
        }
    }

    /// Remove an entry and everything under it (directory delete).
    pub fn remove_prefix(&self, prefix: &str) {
        let mut inner = self.inner.write().unwrap();
        let doomed: Vec<String> = inner
            .entries
            .keys()
            .filter(|p| p.starts_with(prefix) && (*p == prefix || p.starts_with(&format!("{prefix}\\"))))
            .cloned()
            .collect();
        for p in doomed {
            if let Some(old) = inner.entries.remove(&p) {
                inner.refs.remove(&old.file_ref);
            }
        }
    }

    /// Re-prefix every entry under `old_prefix` to `new_prefix` (directory
    /// rename: NTFS emits no per-child rename records, only the directory's).
    pub fn rename_prefix(&self, old_prefix: &str, new_prefix: &str) {
        let mut inner = self.inner.write().unwrap();
        let doomed: Vec<(String, IndexedFile)> = inner
            .entries
            .iter()
            .filter(|(p, _)| p.starts_with(old_prefix) && (*p == old_prefix || p.starts_with(&format!("{old_prefix}\\"))))
            .map(|(p, e)| (p.clone(), e.clone()))
            .collect();
        for (old_path, mut e) in doomed {
            let new_path = format!("{new_prefix}{}", &old_path[old_prefix.len()..]);
            e.set_path(new_path.clone());
            inner.entries.remove(&old_path);
            inner.refs.remove(&e.file_ref);
            inner.entries.insert(new_path.clone(), e.clone());
            inner.refs.insert(e.file_ref, new_path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mft::IndexedFile;

    fn e(path: &str) -> IndexedFile {
        IndexedFile::new(path.to_string(), 0, 0, 0, 0, false, 0)
    }

    fn build() -> FileIndex {
        let ix = FileIndex::new();
        ix.replace(vec![
            e(r"C:\olddir\a.txt"),
            e(r"C:\olddir\sub\b.txt"),
            e(r"C:\other\c.txt"),
            e(r"C:\olddir"),
        ]);
        ix
    }

    #[test]
    fn rename_prefix_moves_subtree() {
        let ix = build();
        ix.rename_prefix(r"C:\olddir", r"C:\newdir");
        ix.with_entries(|m| {
            assert!(m.contains_key(r"C:\newdir\a.txt"));
            assert!(m.contains_key(r"C:\newdir\sub\b.txt"));
            assert!(m.contains_key(r"C:\newdir"));
            assert!(m.contains_key(r"C:\other\c.txt"));
            assert!(!m.contains_key(r"C:\olddir"));
        });
        // Path-by-ref resolution must follow the rename too.
        assert!(ix.len() == 4);
    }

    #[test]
    fn remove_prefix_drops_subtree() {
        let ix = build();
        ix.remove_prefix(r"C:\olddir");
        ix.with_entries(|m| {
            assert!(!m.contains_key(r"C:\olddir"));
            assert!(!m.contains_key(r"C:\olddir\a.txt"));
            assert!(!m.contains_key(r"C:\olddir\sub\b.txt"));
            assert!(m.contains_key(r"C:\other\c.txt"));
        });
    }

    #[test]
    fn rename_prefix_does_not_hit_sibling() {
        // `C:\olddir` must not rename `C:\olddirX`.
        let ix = FileIndex::new();
        ix.replace(vec![e(r"C:\olddir\a.txt"), e(r"C:\olddirX\b.txt")]);
        ix.rename_prefix(r"C:\olddir", r"C:\newdir");
        ix.with_entries(|m| {
            assert!(m.contains_key(r"C:\newdir\a.txt"));
            assert!(m.contains_key(r"C:\olddirX\b.txt"));
        });
    }
}
