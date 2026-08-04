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
