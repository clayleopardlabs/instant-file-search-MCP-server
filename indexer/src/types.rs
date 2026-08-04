//! Portable core types shared by every platform backend.
//!
//! `IndexedFile` is produced by the scan backend (NTFS MFT on Windows,
//! getdents64 + statx walk on Linux), consumed by the in-memory index, the
//! query engine, and the pipe protocol. It must stay free of platform-specific
//! imports so both backends can construct it.

use crate::query::DEFAULT_EXCLUDES;

/// A single indexed file with full path.
#[derive(Debug, Clone)]
pub struct IndexedFile {
    /// Absolute path (`C:\Windows\...` on Windows, `/usr/...` on Linux).
    pub path: String,
    /// File size in bytes (0 for directories).
    pub size: u64,
    /// Creation time, 100 ns since 1601 (FILETIME). On Linux this is the
    /// statx btime converted to the FILETIME epoch.
    pub created: i64,
    /// Last modification time, 100 ns since 1601 (FILETIME).
    pub modified: i64,
    /// Last access time, 100 ns since 1601 (FILETIME).
    pub accessed: i64,
    /// `true` for directories.
    pub is_dir: bool,
    /// Windows FILE_ATTRIBUTE_* flags (query/attrib). On Linux these are
    /// synthesized from the statx mode (see platform backends).
    pub attributes: u32,
    /// File reference: NTFS record number on Windows, inode number on Linux
    /// (used for `frn:` queries and USN/fanotify parent resolution).
    pub file_ref: u64,
    /// Parent record number for THIS link (Windows only: hard links produce
    /// one entry per directory entry; only used during scan-time path
    /// resolution). Unused on Linux.
    pub parent_ref: u64,
    /// File name for THIS link (Windows only: hard links; only used during
    /// scan-time path resolution). Unused on Linux.
    pub own_name: String,
    /// Precomputed lowercase name (query hot path).
    pub name: String,
    /// Precomputed canonical name (NFC + Unicode-lower on macOS; ASCII-lower
    /// elsewhere). The ci matching target for name queries.
    pub lower_name: String,
    /// Precomputed lowercase path (query hot path).
    pub lower_path: String,
    /// Precomputed lowercase extension without the dot (query hot path).
    pub extension: Option<String>,
    /// Precomputed "under a default-excluded dir" (query hot path).
    pub excluded: bool,
}

impl IndexedFile {
    pub fn new(
        path: String,
        size: u64,
        created: i64,
        modified: i64,
        accessed: i64,
        is_dir: bool,
        file_ref: u64,
    ) -> Self {
        let mut f = IndexedFile {
            path,
            size,
            created,
            modified,
            accessed,
            is_dir,
            attributes: 0,
            file_ref,
            parent_ref: 0,
            own_name: String::new(),
            name: String::new(),
            lower_name: String::new(),
            lower_path: String::new(),
            extension: None,
            excluded: false,
        };
        f.refresh();
        f
    }

    fn refresh(&mut self) {
        self.name = self
            .path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        self.lower_name = crate::platform::canonical_key(&self.name);
        self.lower_path = crate::platform::canonical_key(&self.path);
        self.extension = if self.is_dir {
            None
        } else {
            self.path.rsplit_once('.').and_then(|(head, ext)| {
                if head.is_empty() || ext.is_empty() || ext.contains(['\\', '/']) {
                    None
                } else {
                    Some(ext.to_ascii_lowercase())
                }
            })
        };
        self.excluded = is_default_excluded(&self.lower_path);
    }

    pub fn set_path(&mut self, path: String) {
        self.path = path;
        self.refresh();
    }
}

/// True when `lower_path` passes through any default-excluded directory
/// (`windows`, `program files`, `node_modules`, ...) as a full path component.
/// Separator-agnostic: matches both `\` and `/` so the same rule applies on
/// Windows (backslash paths) and Linux (forward-slash paths).
pub fn is_default_excluded(lower_path: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separator_agnostic_name_and_extension() {
        // `name` preserves original case; only `extension`/`lower_path` are
        // lowercased (matches the original mft.rs refresh behavior).
        let w = IndexedFile::new(r"C:\dir\file.TXT".to_string(), 0, 0, 0, 0, false, 0);
        assert_eq!(w.name, "file.TXT");
        assert_eq!(w.extension.as_deref(), Some("txt"));

        let l = IndexedFile::new("/home/user/file.TXT".to_string(), 0, 0, 0, 0, false, 0);
        assert_eq!(l.name, "file.TXT");
        assert_eq!(l.extension.as_deref(), Some("txt"));

        let ld = IndexedFile::new("/home/user/data.dir/f".to_string(), 0, 0, 0, 0, false, 0);
        // The last dot is inside a directory component, so no extension.
        assert_eq!(ld.extension.as_deref(), None);
        // A trailing component containing a dot mid-path is not an extension.
        let ld2 = IndexedFile::new("/home/user/a.dir/b".to_string(), 0, 0, 0, 0, false, 0);
        assert_eq!(ld2.extension.as_deref(), None);
    }

    #[test]
    fn is_default_excluded_both_separators() {
        // DEFAULT_EXCLUDES = node_modules, .git, WinSxS, $Recycle.Bin,
        // System Volume Information. "windows" is NOT in the list.
        assert!(is_default_excluded(r"c:\node_modules\pkg\index.js"));
        assert!(is_default_excluded("/usr/share/node_modules/pkg/index.js"));
        assert!(is_default_excluded(r"c:\$Recycle.Bin\foo"));
        assert!(is_default_excluded("/mnt/c/System Volume Information/foo"));
        assert!(!is_default_excluded("/home/user/windows-like/thing"));
        assert!(!is_default_excluded("/home/user/program.exe"));
        assert!(!is_default_excluded(r"c:\windows\system32\foo.dll"));
    }
}