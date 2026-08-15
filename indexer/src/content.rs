//! Bounded in-memory content index for full-text `content:"..."` search.
//!
//! Everything's `content:` relies on the Windows Search indexer, which is
//! frequently unavailable or stale. The native indexer instead maintains its
//! own bounded content store: a background pass reads eligible files after the
//! main scan (never blocking queries), and the USN watcher keeps it fresh for
//! created/changed/deleted files.
//!
//! Budgeting:
//! - Only files with a text-like extension (or a known no-extension name) and
//!   a size under [`MAX_FILE_BYTES`] are eligible.
//! - Only the first [`MAX_FILE_BYTES`] of each file is retained, lowercased.
//! - Total retained bytes are capped at [`TOTAL_BUDGET`]; once exhausted the
//!   store stops adding new files (existing entries stay searchable).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;
use std::sync::Arc;

use crate::disk::DiskIndex;
use crate::query::DEFAULT_EXCLUDES;

/// Per-file size cap. Only files at or below this are content-indexed, and only
/// the first this-many bytes are retained.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Total retained-content budget across all files.
pub const TOTAL_BUDGET: usize = 256 * 1024 * 1024;

/// Text-like extensions eligible for content indexing (lowercase, no dot).
const TEXT_EXTS: &[&str] = &[
    "md",
    "markdown",
    "txt",
    "rst",
    "log",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "config",
    "env",
    "properties",
    "xml",
    "html",
    "htm",
    "css",
    "csv",
    "tsv",
    "sql",
    "rs",
    "py",
    "pyw",
    "js",
    "jsx",
    "ts",
    "tsx",
    "mjs",
    "cjs",
    "c",
    "h",
    "cpp",
    "cc",
    "cxx",
    "hpp",
    "hh",
    "hxx",
    "cs",
    "java",
    "kt",
    "kts",
    "go",
    "rb",
    "php",
    "sh",
    "bat",
    "cmd",
    "ps1",
    "swift",
    "m",
    "mm",
    "r",
    "scala",
    "clj",
    "cljs",
    "lua",
    "pl",
    "pm",
    "tcl",
    "v",
    "vhdl",
    "asm",
    "s",
    "gradle",
    "groovy",
    "dockerfile",
    "makefile",
    "cmake",
    "gitignore",
    "gitattributes",
    "editorconfig",
    "eslintrc",
    "prettierrc",
    "babelrc",
    "tsconfig",
    "package",
    "lock",
];

/// No-extension names eligible for content indexing (lowercase).
const TEXT_NAMES: &[&str] = &[
    "readme",
    "makefile",
    "dockerfile",
    "license",
    "copying",
    "notice",
    "changelog",
    "authors",
    "contributing",
    "gemfile",
    "rakefile",
    "requirements",
    "rust-toolchain",
    "cargo-config",
];

fn ext_of(path: &str) -> &str {
    let name = match path.rfind('\\').or_else(|| path.rfind('/')) {
        Some(i) => &path[i + 1..],
        None => path,
    };
    match name.rfind('.') {
        Some(i) => &name[i + 1..],
        None => "",
    }
}

fn is_text_file(path: &str) -> bool {
    let ext = ext_of(path);
    if TEXT_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
        return true;
    }
    if ext.is_empty() {
        let name = match path.rfind('\\').or_else(|| path.rfind('/')) {
            Some(i) => &path[i + 1..],
            None => path,
        };
        return TEXT_NAMES.contains(&name.to_ascii_lowercase().as_str());
    }
    false
}

/// Thread-safe bounded content store.
pub struct ContentStore {
    /// Lowercased full path -> lowercased content bytes.
    inner: RwLock<HashMap<String, Vec<u8>>>,
    total: AtomicUsize,
    indexed: AtomicUsize,
    enabled: bool,
    disk: Option<Arc<DiskIndex>>,
}

impl ContentStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            total: AtomicUsize::new(0),
            indexed: AtomicUsize::new(0),
            enabled: true,
            disk: None,
        }
    }

    pub fn disabled() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            total: AtomicUsize::new(0),
            indexed: AtomicUsize::new(0),
            enabled: false,
            disk: None,
        }
    }

    pub fn disk(index: Arc<DiskIndex>) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            total: AtomicUsize::new(0),
            indexed: AtomicUsize::new(0),
            enabled: true,
            disk: Some(index),
        }
    }

    pub fn is_disk(&self) -> bool {
        self.disk.is_some()
    }

    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "storage_mode": if self.is_disk() { "disk" } else if self.is_enabled() { "memory" } else { "off" },
            "enabled": self.is_enabled(),
            "files": self.len(),
            "bytes": self.total_bytes(),
            "budget_bytes": if self.is_disk() { crate::disk::DISK_CONTENT_BUDGET_DEFAULT } else { TOTAL_BUDGET },
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Number of files currently indexed.
    pub fn len(&self) -> usize {
        if let Some(disk) = &self.disk {
            return disk.content_stats().map(|(count, _)| count).unwrap_or(0);
        }
        self.indexed.load(Ordering::Relaxed)
    }

    /// Total retained content bytes.
    pub fn total_bytes(&self) -> usize {
        if let Some(disk) = &self.disk {
            return disk.content_stats().map(|(_, bytes)| bytes).unwrap_or(0);
        }
        self.total.load(Ordering::Relaxed)
    }

    /// True if the path is eligible for content indexing AND is not under a
    /// default-excluded tree.
    pub fn should_index(path: &str, size: u64) -> bool {
        if size == 0 || size > MAX_FILE_BYTES {
            return false;
        }
        if default_excluded(path) {
            return false;
        }
        is_text_file(path)
    }

    /// Store content for a path (path + data already validated eligible).
    /// Keys are canonical (NFC + Unicode-lower on macOS) so they match
    /// `IndexedFile::lower_path` exactly.
    pub fn insert(&self, path: &str, data: &[u8]) {
        if !self.enabled {
            return;
        }
        if data.is_empty() {
            return;
        }
        if let Some(disk) = &self.disk {
            if let Err(error) = disk.content_insert(path, data) {
                tracing::warn!("disk content insert failed for {path}: {error:#}");
            }
            return;
        }
        let mut guard = self.inner.write().unwrap();
        let budget_left = TOTAL_BUDGET.saturating_sub(self.total.load(Ordering::Relaxed));
        if budget_left == 0 {
            // Budget exhausted: refuse new files but keep existing entries.
            return;
        }
        let take = data.len().min(budget_left);
        let key = crate::platform::canonical_key(path);
        let old = guard.remove(&key);
        if let Some(o) = old {
            self.total.fetch_sub(o.len(), Ordering::Relaxed);
            self.indexed.fetch_sub(1, Ordering::Relaxed);
        }
        guard.insert(key, data[..take].to_vec());
        self.total.fetch_add(take, Ordering::Relaxed);
        self.indexed.fetch_add(1, Ordering::Relaxed);
    }

    /// Drop a path from the store (delete / old-name rename).
    pub fn remove(&self, path: &str) {
        if !self.enabled {
            return;
        }
        if let Some(disk) = &self.disk {
            if let Err(error) = disk.content_remove(path) {
                tracing::warn!("disk content removal failed for {path}: {error:#}");
            }
            return;
        }
        let mut guard = self.inner.write().unwrap();
        if let Some(o) = guard.remove(&crate::platform::canonical_key(path)) {
            self.total.fetch_sub(o.len(), Ordering::Relaxed);
            self.indexed.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Set of lowercase paths whose stored content contains every needle
    /// (case-insensitive). Empty needles match nothing.
    pub fn matching_paths(&self, needles: &[String]) -> Vec<String> {
        if needles.is_empty() {
            return Vec::new();
        }
        if self.disk.is_some() {
            return Vec::new();
        }
        let needles: Vec<String> = needles.iter().map(|n| n.to_ascii_lowercase()).collect();
        let guard = self.inner.read().unwrap();
        guard
            .iter()
            .filter(|(_, data)| {
                needles
                    .iter()
                    .all(|n| !n.is_empty() && find_ci(data, n.as_bytes()).is_some())
            })
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// True if the given path's stored content contains `needle`.
    pub fn contains(&self, path: &str, needle: &str) -> bool {
        let needle = needle.to_ascii_lowercase();
        if needle.is_empty() {
            return false;
        }
        if let Some(disk) = &self.disk {
            return disk.content_contains(path, &needle).unwrap_or(false);
        }
        let guard = self.inner.read().unwrap();
        match guard.get(&crate::platform::canonical_key(path)) {
            Some(data) => find_ci(data, needle.as_bytes()).is_some(),
            None => false,
        }
    }
}

/// Case-insensitive substring search (ASCII). Returns the match offset.
fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let n = needle.len();
    let mut i = 0;
    while i + n <= hay.len() {
        if hay[i..i + n].eq_ignore_ascii_case(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// True if the path sits under one of the default-excluded trees.
/// Separator-agnostic (both `\\` and `/`), mirroring types.rs.
fn default_excluded(lower_path: &str) -> bool {
    let bytes = lower_path.as_bytes();
    DEFAULT_EXCLUDES.iter().any(|d| {
        let d = d.as_bytes();
        let mut i = 0;
        while i + d.len() + 1 <= bytes.len() {
            if (bytes[i] == b'\\' || bytes[i] == b'/')
                && bytes[i + 1..i + 1 + d.len()].eq_ignore_ascii_case(d)
            {
                let after = i + 1 + d.len();
                if after == bytes.len() || bytes[after] == b'\\' || bytes[after] == b'/' {
                    return true;
                }
            }
            i += 1;
        }
        false
    })
}

impl Default for ContentStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility() {
        assert!(ContentStore::should_index(r"C:\a\b.rs", 100));
        assert!(ContentStore::should_index(r"C:\a\b.py", 100));
        assert!(ContentStore::should_index(r"C:\a\README", 100));
        assert!(ContentStore::should_index(r"C:\a\Dockerfile", 100));
        assert!(!ContentStore::should_index(r"C:\a\b.exe", 100));
        assert!(!ContentStore::should_index(r"C:\a\b.rs", 0));
        assert!(!ContentStore::should_index(r"C:\a\b.rs", 300 * 1024));
        assert!(!ContentStore::should_index(r"C:\node_modules\x.rs", 100));
        assert!(!ContentStore::should_index(r"C:\a\.git\x.rs", 100));
    }

    #[test]
    fn insert_and_match() {
        let store = ContentStore::new();
        store.insert(
            r"C:\a\hello.rs",
            b"fn main() { println!(\"HELLO WORLD\"); }",
        );
        store.insert(r"C:\a\other.md", b"nothing here");
        assert_eq!(store.len(), 2);

        let hits = store.matching_paths(&["hello".into()]);
        assert_eq!(hits, vec![r"c:\a\hello.rs"]);
        let hits = store.matching_paths(&["Hello World".into()]);
        assert_eq!(hits, vec![r"c:\a\hello.rs"]);
        let hits = store.matching_paths(&["missing".into()]);
        assert!(hits.is_empty());

        assert!(store.contains(r"C:\A\HELLO.RS", "world"));
        assert!(!store.contains(r"C:\a\other.md", "hello"));

        store.remove(r"C:\a\hello.rs");
        assert_eq!(store.len(), 1);
        assert!(store.matching_paths(&["hello".into()]).is_empty());
    }

    #[test]
    fn multiple_needles_all() {
        let store = ContentStore::new();
        store.insert(r"C:\a\x.txt", b"alpha beta gamma");
        store.insert(r"C:\a\y.txt", b"alpha only");
        let hits = store.matching_paths(&["alpha".into(), "gamma".into()]);
        assert_eq!(hits, vec![r"c:\a\x.txt"]);
    }

    #[test]
    fn budget_refuses_new() {
        let store = ContentStore::new();
        // Reserve the whole budget so insert refuses.
        store.insert(r"C:\a\big.txt", &vec![b'a'; TOTAL_BUDGET]);
        assert_eq!(store.len(), 1);
        store.insert(r"C:\a\small.txt", b"hello");
        assert_eq!(
            store.len(),
            1,
            "budget-exhausted store must refuse new files"
        );
        assert!(store.matching_paths(&["hello".into()]).is_empty());
    }
}
