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

use crate::query::DEFAULT_EXCLUDES;

/// Per-file size cap. Only files at or below this are content-indexed, and only
/// the first this-many bytes are retained.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Total retained-content budget across all files.
pub const TOTAL_BUDGET: usize = 256 * 1024 * 1024;

/// Text-like extensions eligible for content indexing (lowercase, no dot).
const TEXT_EXTS: &[&str] = &[
    "md", "markdown", "txt", "rst", "log", "json", "jsonl", "yaml", "yml", "toml", "ini", "cfg",
    "conf", "config", "env", "properties", "xml", "html", "htm", "css", "csv", "tsv", "sql",
    "rs", "py", "pyw", "js", "jsx", "ts", "tsx", "mjs", "cjs", "c", "h", "cpp", "cc", "cxx",
    "hpp", "hh", "hxx", "cs", "java", "kt", "kts", "go", "rb", "php", "sh", "bat", "cmd", "ps1",
    "swift", "m", "mm", "r", "scala", "clj", "cljs", "lua", "pl", "pm", "tcl", "v", "vhdl",
    "asm", "s", "gradle", "groovy", "dockerfile", "makefile", "cmake", "gitignore", "gitattributes",
    "editorconfig", "eslintrc", "prettierrc", "babelrc", "tsconfig", "package", "lock",
];

/// No-extension names eligible for content indexing (lowercase).
const TEXT_NAMES: &[&str] = &[
    "readme", "makefile", "dockerfile", "license", "copying", "notice", "changelog", "authors",
    "contributing", "gemfile", "rakefile", "requirements", "rust-toolchain", "cargo-config",
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
#[derive(Default)]
pub struct ContentStore {
    /// Lowercased full path -> lowercased content bytes.
    inner: RwLock<HashMap<String, Vec<u8>>>,
    total: AtomicUsize,
    indexed: AtomicUsize,
}

impl ContentStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of files currently indexed.
    pub fn len(&self) -> usize {
        self.indexed.load(Ordering::Relaxed)
    }

    /// Total retained content bytes.
    pub fn total_bytes(&self) -> usize {
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
    pub fn insert(&self, path: &str, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let mut guard = self.inner.write().unwrap();
        let budget_left = TOTAL_BUDGET.saturating_sub(self.total.load(Ordering::Relaxed));
        if budget_left == 0 {
            // Budget exhausted: refuse new files but keep existing entries.
            return;
        }
        let take = data.len().min(budget_left);
        let old = guard.remove(&path.to_ascii_lowercase());
        if let Some(o) = old {
            self.total.fetch_sub(o.len(), Ordering::Relaxed);
            self.indexed.fetch_sub(1, Ordering::Relaxed);
        }
        guard.insert(path.to_ascii_lowercase(), data[..take].to_vec());
        self.total.fetch_add(take, Ordering::Relaxed);
        self.indexed.fetch_add(1, Ordering::Relaxed);
    }

    /// Drop a path from the store (delete / old-name rename).
    pub fn remove(&self, path: &str) {
        let mut guard = self.inner.write().unwrap();
        if let Some(o) = guard.remove(&path.to_ascii_lowercase()) {
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
        let needles: Vec<String> = needles.iter().map(|n| n.to_ascii_lowercase()).collect();
        let guard = self.inner.read().unwrap();
        guard
            .iter()
            .filter(|(_, data)| {
                needles.iter().all(|n| {
                    !n.is_empty() && find_ci(data, n.as_bytes()).is_some()
                })
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
        let guard = self.inner.read().unwrap();
        match guard.get(&path.to_ascii_lowercase()) {
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
fn default_excluded(lower_path: &str) -> bool {
    let bytes = lower_path.as_bytes();
    DEFAULT_EXCLUDES.iter().any(|d| {
        let d = d.as_bytes();
        let mut i = 0;
        while i + d.len() + 1 <= bytes.len() {
            if bytes[i] == b'\\' && bytes[i + 1..i + 1 + d.len()].eq_ignore_ascii_case(d) {
                let after = i + 1 + d.len();
                if after == bytes.len() || bytes[after] == b'\\' {
                    return true;
                }
            }
            i += 1;
        }
        false
    })
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
        store.insert(r"C:\a\hello.rs", b"fn main() { println!(\"HELLO WORLD\"); }");
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
        assert_eq!(store.len(), 1, "budget-exhausted store must refuse new files");
        assert!(store.matching_paths(&["hello".into()]).is_empty());
    }
}
