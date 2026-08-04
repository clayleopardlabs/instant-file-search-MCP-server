//! Platform seam: the set of operations that differ between the Windows and
//! Linux backends. Everything above this module (`index`, `query`, `content`,
//! `pipe` protocol) is portable.
//!
//! Windows backend: NTFS `$MFT` raw scan + USN Change Journal watcher.
//! Linux backend: getdents64/statx directory walk + fanotify watcher.

#[cfg(windows)]
pub use crate::mft::{discover_volumes, scan_volume};
#[cfg(windows)]
pub use crate::usn::{journal_tails, watch_all};

#[cfg(target_os = "linux")]
pub use crate::walk::{discover_volumes, scan_volume, volume_of};
#[cfg(target_os = "linux")]
pub use crate::fanotify::{journal_tails, watch_all};

#[cfg(target_os = "macos")]
pub use crate::walk_macos::{discover_volumes, scan_volume, volume_of};
#[cfg(target_os = "macos")]
pub use crate::fsevents::{journal_tails, watch_all};

#[cfg(windows)]
pub use crate::pipe::PipeServer;
#[cfg(not(windows))]
pub use crate::pipe_unix::PipeServer;

/// Native path separator for this platform, as a string usable in `replace`.
#[cfg(windows)]
pub const SEP: char = '\\';
#[cfg(not(windows))]
pub const SEP: char = '/';

/// Normalize a user-supplied path into the native separator form used by
/// indexed paths. Windows stores `C:\...` (backslashes); Linux stores
/// `/usr/...` (forward slashes). Accepts both forms from MCP callers and
/// converts to the native form.
#[cfg(windows)]
pub fn normalize_path(p: &str) -> String {
    p.replace('/', "\\")
}
#[cfg(not(windows))]
pub fn normalize_path(p: &str) -> String {
    p.replace('\\', "/")
}

/// Canonical key for case-insensitive matching.
///
/// macOS APFS is normalization- and case-insensitive but byte-preserving:
/// readdir can return a name in NFC or NFD (or a mix within one path), so
/// byte-exact `to_ascii_lowercase` matching would silently miss the other
/// form. Both the index key (`lower_path`) and every query pattern must be
/// run through this so byte-wise matching sees identical canonical bytes.
/// Windows/Linux keep ASCII-lowercasing (identity for ASCII paths; unchanged
/// behavior — NTFS and ext4 do not mix normalization forms on disk).
#[cfg(target_os = "macos")]
pub fn canonical_key(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().collect::<String>().to_lowercase()
}
#[cfg(not(target_os = "macos"))]
pub fn canonical_key(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// NFC-normalize a string, preserving case. Used for case-sensitive matching
/// on macOS, where APFS is normalization-insensitive but byte-preserving: a
/// `case:` pattern and the stored name must agree on NFC form or byte-exact
/// comparison misses. Identity elsewhere (Windows/Linux store one form).
#[cfg(target_os = "macos")]
pub fn nfc_normalize(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().collect()
}
#[cfg(not(target_os = "macos"))]
pub fn nfc_normalize(s: &str) -> String {
    s.to_string()
}

/// Canonical form for a query pattern that will be matched with the given
/// case-sensitivity: NFC + Unicode-lowercase for case-insensitive matching,
/// NFC-only (case preserved) for case-sensitive matching. Windows/Linux
/// resolve to ASCII-lowercase / identity respectively, matching the existing
/// byte-wise helpers.
pub fn pattern_key(s: &str, cs: bool) -> String {
    if cs {
        nfc_normalize(s)
    } else {
        canonical_key(s)
    }
}

/// Trim a trailing separator so scope comparisons against `C:\` vs `C:`
/// (Windows) or `/` vs `` (Linux) behave. The index's volume keys and path
/// components never carry a trailing separator except at the filesystem root.
pub fn trim_trailing_sep(p: &str) -> &str {
    if p.len() > 1 && p.ends_with(SEP) {
        &p[..p.len() - 1]
    } else {
        p
    }
}

/// True when `path` is an absolute path (drive-rooted on Windows, `/` on
/// Linux). Used to decide whether a bare token is a path filter.
#[cfg(windows)]
pub fn is_absolute_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}
#[cfg(not(windows))]
pub fn is_absolute_path(p: &str) -> bool {
    p.starts_with('/')
}

/// Extract the volume/root identifier from an absolute path.
/// Windows: `C:\Windows\x` -> `C:`. Linux: mount-aware, handled by `walk`
/// (inodes are only unique per filesystem, so the volume must be the mount
/// root the path lives under, not a fixed `/`).
/// Bare names (no separator) are returned unchanged.
#[cfg(windows)]
pub fn volume_of(path: &str) -> String {
    path.split('\\')
        .next()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Parent path of `path` (the directory containing it), or None for roots.
#[cfg(windows)]
pub fn parent_of(path: &str) -> Option<String> {
    let norm = normalize_path(path);
    let idx = norm.rfind('\\')?;
    let parent = &norm[..idx];
    if parent.is_empty() {
        // `C:\` has no parent.
        None
    } else {
        Some(parent.to_string())
    }
}
#[cfg(not(windows))]
pub fn parent_of(path: &str) -> Option<String> {
    let idx = path.rfind('/')?;
    let parent = &path[..idx];
    if parent.is_empty() {
        None
    } else {
        Some(parent.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_key_ci_lowercases_cs_preserves() {
        // ci: canonical (lowercase); cs: NFC-only (case preserved).
        assert_eq!(pattern_key("FooBar", false), "foobar");
        assert_eq!(pattern_key("FooBar", true), "FooBar");
    }

    #[test]
    fn normalize_path_converts_foreign_separators() {
        #[cfg(windows)]
        {
            assert_eq!(normalize_path("C:/Users/me"), r"C:\Users\me");
            assert_eq!(normalize_path(r"C:\Users\me"), r"C:\Users\me");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(normalize_path(r"C:\Users\me"), "C:/Users/me");
            assert_eq!(normalize_path("/Users/me"), "/Users/me");
        }
    }

    #[test]
    fn trim_trailing_sep_keeps_root() {
        // Trims the NATIVE separator only (callers normalize first).
        #[cfg(windows)]
        {
            assert_eq!(trim_trailing_sep(r"C:\"), r"C:");
            assert_eq!(trim_trailing_sep(r"c:\users\"), r"c:\users");
            // A foreign `/` trailing separator is left alone on Windows.
            assert_eq!(trim_trailing_sep("/Users/me/"), "/Users/me/");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(trim_trailing_sep("/"), "/");
            assert_eq!(trim_trailing_sep("/Users/me/"), "/Users/me");
            // A foreign `\\` trailing separator is left alone on Unix.
            assert_eq!(trim_trailing_sep(r"C:\"), r"C:\");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn canonical_key_nfc_unicode_lower() {
        // NFD "café" (e + combining acute) and NFC "café" (é) must collide.
        let nfd = "cafe\u{301}";
        let nfc = "caf\u{e9}";
        assert_ne!(nfd, nfc, "NFD and NFC forms differ as bytes");
        assert_eq!(canonical_key(nfd), canonical_key(nfc));
        // Unicode lowercase: "CAFÉ" -> "café".
        assert_eq!(canonical_key("CAF\u{e9}"), "caf\u{e9}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nfc_normalize_preserves_case() {
        assert_eq!(nfc_normalize("CAF\u{e9}"), "CAF\u{e9}");
        assert_eq!(nfc_normalize("cafe\u{301}"), "caf\u{e9}");
    }
}
