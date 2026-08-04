//! In-memory file index shared between the scanner, USN watcher, and query engine.

use std::collections::VecDeque;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::types::IndexedFile;

/// One recorded change event (populated from the USN journal).
#[derive(Clone, Serialize, Debug)]
pub struct ChangeEvent {
    /// Windows FILETIME (100ns since 1601) of the change.
    pub timestamp: i64,
    /// Human-readable reason (e.g. "CREATE", "DELETE", "RENAME", "CLOSE").
    pub reason: String,
    /// Full path affected.
    pub path: String,
    /// Whether the affected entry is a directory.
    pub is_dir: bool,
}

/// Thread-safe index of every known file.
#[derive(Default)]
pub struct FileIndex {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Full path -> entry
    entries: HashMap<String, IndexedFile>,
    /// (volume, MFT record number) -> full path (USN parent resolution).
    /// Record numbers are only unique WITHIN a volume, so the volume is part
    /// of the key; keying by record number alone made C:/D:/E: entries with
    /// the same record number collide (wrong parent paths in recent_changes).
    refs: HashMap<(String, u64), String>,
    /// Bounded ring buffer of recent change events (USN watcher).
    changes: VecDeque<ChangeEvent>,
}

/// Maximum number of change events retained in memory.
const MAX_CHANGES: usize = 10_000;

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
            inner
                .refs
                .insert((volume_of(&e.path), e.file_ref), e.path.clone());
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
        let pairs: Vec<((String, u64), String)> = inner
            .entries
            .iter()
            .map(|(path, e)| ((volume_of(path), e.file_ref), path.clone()))
            .collect();
        for (k, p) in pairs {
            inner.refs.insert(k, p);
        }
        for e in entries {
            inner
                .refs
                .insert((volume_of(&e.path), e.file_ref), e.path.clone());
            inner.entries.insert(e.path.clone(), e);
        }
        inner.entries.len()
    }

    /// Insert or update one entry.
    ///
    /// Directory entries store their recursive (tree-summed) size, not the
    /// directory's own allocation. A USN stat only knows the latter, so an
    /// existing directory keeps its stored total and a brand-new one is
    /// seeded from whatever children are already indexed. Files propagate a
    /// size delta up to every ancestor directory so `size:` filters stay in
    /// sync with the tree.
    pub fn upsert(&self, entry: IndexedFile) {
        let mut inner = self.inner.write().unwrap();
        let old = inner.entries.get(&entry.path).cloned();
        let old_size = old.as_ref().map(|e| e.size).unwrap_or(0);
        let old_is_dir = old.as_ref().map(|e| e.is_dir).unwrap_or(false);
        let mut entry = entry;
        let mut delta: i64 = 0;
        if entry.is_dir {
            if old_is_dir {
                entry.size = old_size;
            } else {
                entry.size = sum_children(&inner, &entry.path);
                // The old file's contribution is gone once the dir replaces it.
                delta = -(old_size as i64);
            }
        } else {
            delta = entry.size as i64 - old_size as i64;
        }
        let old_ref = inner.entries.get(&entry.path).map(|old| old.file_ref);
        if let Some(r) = old_ref {
            inner.refs.remove(&(volume_of(&entry.path), r));
        }
        inner
            .refs
            .insert((volume_of(&entry.path), entry.file_ref), entry.path.clone());
        let path = entry.path.clone();
        inner.entries.insert(path.clone(), entry);
        adjust_ancestors(&mut inner, &path, delta);
        tracing::debug!("upsert: entries={} refs={}", inner.entries.len(), inner.refs.len());
    }

    /// Remove an entry by full path (file delete). Directory deletes go
    /// through `remove_prefix`.
    pub fn remove(&self, path: &str) {
        let mut inner = self.inner.write().unwrap();
        if let Some(old) = inner.entries.remove(path) {
            inner.refs.remove(&(volume_of(path), old.file_ref));
            if !old.is_dir {
                adjust_ancestors(&mut inner, path, -(old.size as i64));
            }
        }
    }

    /// Record a change event into the bounded ring buffer.
    pub fn record_change(&self, timestamp: i64, reason: &str, path: &str, is_dir: bool) {
        let mut inner = self.inner.write().unwrap();
        inner.changes.push_back(ChangeEvent {
            timestamp,
            reason: reason.to_string(),
            path: path.to_string(),
            is_dir,
        });
        while inner.changes.len() > MAX_CHANGES {
            inner.changes.pop_front();
        }
    }

    /// Return change events since a timestamp (exclusive), newest-first.
    /// `limit` caps the result (0 = no cap, bounded by the ring buffer size).
    pub fn recent_changes(&self, since: i64, limit: usize) -> Vec<ChangeEvent> {
        let inner = self.inner.read().unwrap();
        inner
            .changes
            .iter()
            .rev()
            .filter(|c| c.timestamp > since)
            .take(if limit == 0 { usize::MAX } else { limit })
            .cloned()
            .collect()
    }

    /// Like [`recent_changes`](Self::recent_changes) but also filters by a
    /// comma-separated `reasons` list. Accepted values (case-insensitive):
    /// created, modified, renamed, deleted. `None`/empty returns everything.
    pub fn recent_changes_filtered(&self, since: i64, limit: usize, reasons: Option<&str>) -> Vec<ChangeEvent> {
        let wants = |reason: &str| -> bool {
            let Some(list) = reasons else { return true };
            if list.trim().is_empty() {
                return true;
            }
            let cat = match reason {
                "DELETE" => "deleted",
                "RENAME" | "RENAME_NEW" => "renamed",
                _ => "modified", // CREATE and CLOSE both surface as WRITE
            };
            list.split(',').any(|w| w.trim().eq_ignore_ascii_case(cat))
        };
        let inner = self.inner.read().unwrap();
        inner
            .changes
            .iter()
            .rev()
            .filter(|c| c.timestamp > since && wants(&c.reason))
            .take(if limit == 0 { usize::MAX } else { limit })
            .cloned()
            .collect()
    }

    /// Remove an entry and everything under it (directory delete). The whole
    /// subtree vanishes at once, so only the ancestors of `prefix` need their
    /// recursive total reduced by the subtree's size.
    pub fn remove_prefix(&self, prefix: &str) {
        let mut inner = self.inner.write().unwrap();
        let root_total = inner.entries.get(prefix).map(|e| e.size).unwrap_or(0);
        let doomed: Vec<String> = inner
            .entries
            .keys()
            .filter(|p| p.starts_with(prefix) && (*p == prefix || p.starts_with(&format!("{prefix}\\"))))
            .cloned()
            .collect();
        for p in doomed {
            if let Some(old) = inner.entries.remove(&p) {
                inner.refs.remove(&(volume_of(&p), old.file_ref));
            }
        }
        adjust_ancestors(&mut inner, prefix, -(root_total as i64));
    }

    /// Re-prefix every entry under `old_prefix` to `new_prefix` (directory
    /// rename: NTFS emits no per-child rename records, only the directory's).
    /// The subtree keeps its internal sizes, so only the ancestors of the old
    /// and new roots change: old loses the subtree total, new gains it.
    pub fn rename_prefix(&self, old_prefix: &str, new_prefix: &str) {
        let mut inner = self.inner.write().unwrap();
        let root_total = inner.entries.get(old_prefix).map(|e| e.size).unwrap_or(0);
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
            inner.refs.remove(&(volume_of(&old_path), e.file_ref));
            inner.entries.insert(new_path.clone(), e.clone());
            inner.refs.insert((volume_of(&new_path), e.file_ref), new_path);
        }
        adjust_ancestors(&mut inner, old_prefix, -(root_total as i64));
        adjust_ancestors(&mut inner, new_prefix, root_total as i64);
    }

    /// Resolve an MFT record number to a full path on a specific volume (USN
    /// parent resolution). The volume is required because NTFS record numbers
    /// are only unique within a volume.
    ///
    /// The volume string is normalized (trailing backslash trimmed) so both
    /// `C:` and `C:\` resolve: `discover_ntfs_volumes` yields `C:\` while
    /// `volume_of` on an indexed path yields `C:`. Without this, every USN
    /// parent lookup misses the refs map and falls back to a bare-root path.
    pub fn path_by_ref(&self, volume: &str, file_ref: u64) -> Option<String> {
        let vol = volume.trim_end_matches('\\');
        self.inner
            .read()
            .unwrap()
            .refs
            .get(&(vol.to_string(), file_ref))
            .cloned()
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

/// Extract the volume root (`C:`) from a full path like `C:\Users\x`. Paths
/// without a drive letter (bare names) are returned unchanged.
fn volume_of(path: &str) -> String {
    crate::platform::volume_of(path)
}

/// Propagate a size delta up every ancestor directory of `path`, keeping the
/// stored recursive totals in sync. Walks the path components from the direct
/// parent up to the volume root (`C:\a\b\f.txt` touches `C:\a\b`, `C:\a`,
/// `C:`). Stops at the volume root (no backslash).
fn adjust_ancestors(inner: &mut Inner, path: &str, delta: i64) {
    if delta == 0 {
        return;
    }
    let mut p = path;
    while let Some(idx) = p.rfind('\\') {
        p = &p[..idx];
        if let Some(e) = inner.entries.get_mut(p) {
            e.size = (e.size as i64 + delta).max(0) as u64;
        }
    }
}

/// Sum the stored sizes of the DIRECT children of `dir_path` (files by their
/// own size, subdirectories by their recursive total). Used to seed a new
/// directory's recursive total from children already in the index.
fn sum_children(inner: &Inner, dir_path: &str) -> u64 {
    let prefix = format!("{dir_path}\\");
    inner
        .entries
        .iter()
        .filter(|(p, _)| p.starts_with(&prefix) && !p[prefix.len()..].contains('\\'))
        .map(|(_, e)| e.size)
        .sum()
}

/// Convenience alias for `Arc<FileIndex>`.
pub type SharedIndex = Arc<FileIndex>;

#[cfg(test)]
mod tests {
    use super::*;
use crate::types::IndexedFile;

/// Maximum number of recent change events retained in the ring buffer.
const MAX_CHANGES: usize = 100_000;

    fn e(path: &str) -> IndexedFile {
        IndexedFile::new(path.to_string(), 0, 0, 0, 0, false, 0)
    }

    fn edir(path: &str, size: u64) -> IndexedFile {
        let mut d = e(path);
        d.is_dir = true;
        d.size = size;
        d
    }

    fn efile(path: &str, size: u64) -> IndexedFile {
        let mut f = e(path);
        f.size = size;
        f
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
    fn change_log_ring_buffers_and_orders() {
        let ix = FileIndex::new();
        ix.record_change(100, "CREATE", r"C:\a.txt", false);
        ix.record_change(200, "RENAME", r"C:\b.txt", false);
        ix.record_change(300, "DELETE", r"C:\a.txt", false);
              let log = ix.recent_changes(0, 0);
              assert_eq!(log.len(), 3);
              assert_eq!(log[0].reason, "DELETE");
              assert_eq!(log[2].reason, "CREATE");
              // since filter: strictly newer than 100 -> last two, newest first
              let filtered = ix.recent_changes(100, 0);
              assert_eq!(filtered.len(), 2);
              assert_eq!(filtered[0].timestamp, 300);
              // limit caps from the newest end
              let capped = ix.recent_changes(0, 2);
              assert_eq!(capped.len(), 2);
              assert_eq!(capped[0].timestamp, 300);
    }

    #[test]
    fn adjust_ancestors_propagates_delta() {
        let ix = FileIndex::new();
        // C:\a\b\f.txt (20) + C:\a\g.txt (30). Dir sizes are recursive totals.
        ix.replace(vec![
            edir(r"C:", 50),
            edir(r"C:\a", 50),
            edir(r"C:\a\b", 20),
            efile(r"C:\a\b\f.txt", 20),
            efile(r"C:\a\g.txt", 30),
        ]);
        // Adding a 5-byte file under C:\a\b\ bumps C:\a\b, C:\a, C:.
        ix.upsert(efile(r"C:\a\b\h.txt", 5));
        ix.with_entries(|m| {
            assert_eq!(m[r"C:\a\b"].size, 25);
            assert_eq!(m[r"C:\a"].size, 55);
            assert_eq!(m[r"C:"].size, 55);
        });
        // Removing a 30-byte file shrinks its ancestors.
        ix.remove(r"C:\a\g.txt");
        ix.with_entries(|m| {
            assert_eq!(m[r"C:\a"].size, 25);
            assert_eq!(m[r"C:"].size, 25);
        });
    }

    #[test]
    fn upsert_keeps_dir_recursive_total() {
        // A USN re-stat of a directory must NOT clobber its recursive total
        // with the dir's own allocation (e.g. 0): the recursive size stays.
        let ix = FileIndex::new();
        ix.replace(vec![
            edir(r"C:", 100),
            edir(r"C:\a", 100),
            efile(r"C:\a\f.txt", 100),
        ]);
        let mut fresh_stat = edir(r"C:\a", 0); // stat reports dir allocation 0
        fresh_stat.created = 1; // force timestamps to differ from existing entry
        ix.upsert(fresh_stat);
        ix.with_entries(|m| {
            assert_eq!(m[r"C:\a"].size, 100, "dir recursive total must persist");
        });
        // A new empty directory starts at recursive size 0.
        let ix2 = FileIndex::new();
        ix2.replace(vec![edir(r"C:", 0)]);
        ix2.upsert(edir(r"C:\empty", 0));
        ix2.with_entries(|m| {
            assert_eq!(m[r"C:\empty"].size, 0);
        });
    }

    #[test]
    fn remove_prefix_subtracts_recursive_size() {
        let ix = FileIndex::new();
        // C: = C:\a (120, subtree 100+20ish) + C:\b (20) = 140.
        ix.replace(vec![
            edir(r"C:", 140),
            edir(r"C:\a", 120),
            edir(r"C:\a\sub", 100),
            efile(r"C:\a\sub\f.bin", 100),
            edir(r"C:\b", 20),
            efile(r"C:\b\g.bin", 20),
        ]);
        ix.remove_prefix(r"C:\a");
        ix.with_entries(|m| {
            assert!(!m.contains_key(r"C:\a"));
            assert_eq!(m[r"C:"].size, 20, r"C: loses the C:\a subtree total");
        });
    }

    #[test]
    fn rename_prefix_moves_recursive_size() {
        let ix = FileIndex::new();
        // C: = C:\a (120) + C:\b (20) = 140; a rename keeps C:'s total.
        ix.replace(vec![
            edir(r"C:", 140),
            edir(r"C:\a", 120),
            edir(r"C:\a\sub", 100),
            efile(r"C:\a\sub\f.bin", 100),
            edir(r"C:\b", 20),
            efile(r"C:\b\g.bin", 20),
        ]);
        ix.rename_prefix(r"C:\a", r"C:\renamed");
        ix.with_entries(|m| {
            assert_eq!(m[r"C:\renamed"].size, 120, "dir keeps recursive total");
            assert_eq!(m[r"C:"].size, 140, "C: total unchanged by rename");
            assert!(!m.contains_key(r"C:\a"));
        });
    }

    #[test]
    fn path_by_ref_is_volume_scoped() {
        // Record numbers are only unique per-volume. Two volumes can both
        // contain record 0x1001; the refs map must keep them separate or a
        // USN parent-FRN lookup on C: can resolve to an unrelated D: path
        // (the cross-volume collision that produced "file.cat\\child.log").
        let ix = FileIndex::new();
        let mut d = edir(r"C:\Users\sophi", 0);
        d.file_ref = 0x1001;
        let mut e = efile(r"D:\Windows\servicing\x.cat", 0);
        e.file_ref = 0x1001;
        ix.replace(vec![d, e]);
        assert_eq!(ix.path_by_ref(r"C:", 0x1001), Some(r"C:\Users\sophi".to_string()));
        assert_eq!(ix.path_by_ref(r"D:", 0x1001), Some(r"D:\Windows\servicing\x.cat".to_string()));
        // Unscoped/unknown volume must NOT resolve to the other volume's path.
        assert_eq!(ix.path_by_ref(r"E:", 0x1001), None);
        // The USN watcher passes volume strings WITH a trailing backslash
        // (discover_ntfs_volumes yields "C:\"); the normalized lookup must
        // resolve them to the same entries as the bare "C:" form.
        assert_eq!(ix.path_by_ref(r"C:\", 0x1001), Some(r"C:\Users\sophi".to_string()));
        assert_eq!(ix.path_by_ref(r"D:\", 0x1001), Some(r"D:\Windows\servicing\x.cat".to_string()));
    }

    #[test]
    fn recent_changes_filters_by_reason() {
        let ix = FileIndex::new();
        ix.record_change(100, "CREATE", r"C:\a\new.txt", false);
        ix.record_change(200, "WRITE", r"C:\a\changed.txt", false);
        ix.record_change(300, "RENAME", r"C:\a\old.txt", false);
        ix.record_change(400, "RENAME_NEW", r"C:\a\newer.txt", false);
        ix.record_change(500, "DELETE", r"C:\a\gone.txt", false);
        // No filter: everything back, newest-first.
        let all = ix.recent_changes_filtered(0, 0, None);
        assert_eq!(all.len(), 5);
        // Newest-first: the most recent event (DELETE, ts 500) comes first.
        assert_eq!(all[0].reason, "DELETE");
        assert_eq!(all[0].path, r"C:\a\gone.txt");
        // Filter to created/modified only: CREATE and WRITE both map to "modified".
        let cm = ix.recent_changes_filtered(0, 0, Some("created,modified"));
        let reasons: Vec<_> = cm.iter().map(|c| c.reason.as_str()).collect();
        assert_eq!(reasons, vec!["WRITE", "CREATE"]);
        // Deleted only.
        let del = ix.recent_changes_filtered(0, 0, Some("deleted"));
        assert_eq!(del.len(), 1);
        assert_eq!(del[0].reason, "DELETE");
        // Renamed covers both RENAME and RENAME_NEW.
        let ren = ix.recent_changes_filtered(0, 0, Some("renamed"));
        assert_eq!(ren.len(), 2);
        // Empty string behaves like None (no filtering).
        assert_eq!(ix.recent_changes_filtered(0, 0, Some("   ")).len(), 5);
        // Case-insensitive.
        assert_eq!(ix.recent_changes_filtered(0, 0, Some("DELETED")).len(), 1);
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
