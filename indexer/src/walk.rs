//! Linux enumeration backend for the instant-file-search indexer.
//!
//! Uses getdents64 (via rustix `Dir`) + `statx` to walk directory trees,
//! building `IndexedFile` entries with inode as `file_ref`, btime as creation
//! time, and attributes synthesized from the statx mode.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use rustix::fs::{
    AtFlags, Dir, Mode, OFlags, Statx, StatxFlags, CWD,
};
use rustix::fs::open as rustix_open;

use crate::types::IndexedFile;

// ---------------------------------------------------------------------------
// FILE_ATTRIBUTE_* constants (Windows semantics, synthesized on Linux).
// Matches the `attrib:` query filter in query.rs.
// ---------------------------------------------------------------------------
const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;

// statx mode masks (linux kernel S_IFMT / S_IFREG).
const S_IFMT: u16 = 0o170000;
const S_IFREG: u16 = 0o100000;

// Windows FILETIME epoch offset: seconds between 1601-01-01 and 1970-01-01.
const FILETIME_EPOCH_OFFSET: i64 = 116_444_73600;

/// Filesystem types considered "real disk" volumes.
/// Pseudo/virtual/virtualised types are excluded.
const FSTYPE_WHITELIST: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "f2fs", "vfat", "exfat",
    "ntfs", "ntfs3", "fuseblk", "zfs",
];

// ---------------------------------------------------------------------------
// Mount-point cache (populated once per process).
// ---------------------------------------------------------------------------
fn cached_mounts() -> &'static Vec<String> {
    static MOUNTS: OnceLock<Vec<String>> = OnceLock::new();
    MOUNTS.get_or_init(|| discover_volumes_inner())
}

/// Combine statx major/minor into a single u64 for device comparison.
fn dev_id(stx: &Statx) -> u64 {
    ((stx.stx_dev_major as u64) << 32) | (stx.stx_dev_minor as u64)
}

/// Convert a `StatxTimestamp` to Windows FILETIME (100ns since 1601).
/// Returns 0 if tv_sec is 0 (timestamp unavailable).
fn to_filetime(ts: rustix::fs::StatxTimestamp) -> i64 {
    if ts.tv_sec == 0 {
        return 0;
    }
    (ts.tv_sec + FILETIME_EPOCH_OFFSET) * 10_000_000 + (ts.tv_nsec as i64) / 100
}

/// Derive Windows FILE_ATTRIBUTE_* flags from a statx mode and file name.
fn synthesize_attributes(mode: u16, is_dir: bool, name: &str) -> u32 {
    let mut attrs = 0u32;
    if is_dir {
        attrs |= FILE_ATTRIBUTE_DIRECTORY;
    }
    if name.starts_with('.') {
        attrs |= FILE_ATTRIBUTE_HIDDEN;
    }
    if mode & 0o222 == 0 {
        attrs |= FILE_ATTRIBUTE_READONLY;
    }
    // ARCHIVE is the Windows default for regular files (harmless on Linux).
    if (mode & S_IFMT) == S_IFREG {
        attrs |= FILE_ATTRIBUTE_ARCHIVE;
    }
    attrs
}

/// Open a directory for iteration.
fn open_dir(path: &Path) -> Result<Dir> {
    let fd = rustix_open(path, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
        .with_context(|| format!("open directory {}", path.display()))?;
    Dir::new(fd).with_context(|| format!("Dir::new for {}", path.display()))
}

// ===========================================================================
// Public API
// ===========================================================================

/// Real-disk mount points, deepest-first, deduplicated.
///
/// Parses `/proc/self/mounts`, filters by fstype whitelist, deduplicates by
/// device (keeping the longest / most-specific mount point per device), and
/// returns them sorted with `/` first.
pub fn discover_volumes() -> Vec<String> {
    // Delegate to the uncached inner implementation via the OnceLock cache.
    // The cache is used by volume_of; discover_volumes always returns the
    // fresh list from the cache (mounts don't change within a process).
    cached_mounts().clone()
}

/// Walk one mount point recursively, returning every file and directory
/// as an IndexedFile.
///
/// Does NOT descend into other mounts (cross-device boundary) and does NOT
/// follow symlinks.
pub fn scan_volume(volume: &str) -> Result<Vec<IndexedFile>> {
    let root_path = PathBuf::from(volume);
    let root_dir = open_dir(&root_path)?;

    // Stat the root to get its device id (for cross-mount guard).
    let root_stat = statx_follow(&root_dir, ".").context("statx root")?;
    let root_dev = dev_id(&root_stat);

    let mut entries: Vec<IndexedFile> = Vec::new();

    // Iterative DFS using an explicit work stack to avoid stack depth issues
    // on deep trees (Windows scan is iterative; parallel-by-subtree is a
    // documented future perf step).
    let mut stack: Vec<(PathBuf, Dir)> = vec![(root_path, root_dir)];

    while let Some((dir_path, mut dir)) = stack.pop() {
        loop {
            match dir.read() {
                Some(Ok(entry)) => {
                    let name_bytes = entry.file_name().to_bytes();

                    // Skip "." and "..".
                    if name_bytes == b"." || name_bytes == b".." {
                        continue;
                    }

                    let ft = entry.file_type();

                    // Skip symlinks entirely for v1 (avoids cycles and
                    // duplicate inode weirdness).
                    if ft.is_symlink() {
                        continue;
                    }

                    let name = match entry.file_name().to_str() {
                        Ok(s) => s.to_owned(),
                        Err(_) => continue, // skip non-UTF-8 names
                    };

                    let full_path = format!("{}/{}", dir_path.display(), name);

                    // Stat the entry (SYMLINK_NOFOLLOW for race safety,
                    // even though we already filtered symlinks via d_type).
                    let path_cstr = CString::new(full_path.as_bytes()).unwrap_or_else(|_| {
                        // Fallback: shouldn't happen with valid paths, but
                        // handle null bytes gracefully.
                        CString::default()
                    });
                    if path_cstr.to_bytes().is_empty() {
                        continue;
                    }

                    let stat = match statx_nofollow(&path_cstr) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!("statx failed for {}: {e}", full_path);
                            continue;
                        }
                    };

                    let is_dir = ft.is_dir();
                    let entry_dev = dev_id(&stat);

                    // Cross-device guard: if the entry is a directory whose
                    // device differs from the volume root, do not descend.
                    if is_dir && entry_dev != root_dev {
                        continue;
                    }

                    // Determine creation time: prefer btime, fall back to
                    // ctime, then mtime.
                    let stx_mask = stat.stx_mask;
                    let has_btime = (stx_mask & StatxFlags::BTIME.bits()) != 0;
                    let created = if has_btime && stat.stx_btime.tv_sec != 0 {
                        to_filetime(stat.stx_btime)
                    } else if stat.stx_ctime.tv_sec != 0 {
                        to_filetime(stat.stx_ctime)
                    } else {
                        to_filetime(stat.stx_mtime)
                    };
                    let modified = to_filetime(stat.stx_mtime);
                    let accessed = to_filetime(stat.stx_atime);

                    let size = if is_dir { 0 } else { stat.stx_size };
                    let inode = stat.stx_ino;
                    let attrs = synthesize_attributes(stat.stx_mode, is_dir, &name);

                    let mut file = IndexedFile::new(
                        full_path,
                        size,
                        created,
                        modified,
                        accessed,
                        is_dir,
                        inode,
                    );
                    file.attributes = attrs;

                    entries.push(file);

                    // Push directories onto the work stack for further
                    // iteration.
                    if is_dir {
                        let child_path = dir_path.join(&name);
                        match open_dir(&child_path) {
                            Ok(child_dir) => {
                                stack.push((child_path, child_dir));
                            }
                            Err(e) => {
                                tracing::warn!("open dir failed for {}: {e}", child_path.display());
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!("readdir error in {}: {e}", dir_path.display());
                    break;
                }
                None => break, // end of directory
            }
        }
    }

    Ok(entries)
}

/// Stat a single path into an `IndexedFile` (used by the fanotify watcher
/// to re-index files on CREATE / MODIFY / RENAME events). Returns None if
/// the path no longer exists or cannot be stat'ed.
///
/// Mirrors the per-entry logic of `scan_volume` for one path: inode becomes
/// `file_ref`, btime (fallback ctime, mtime) becomes the creation time, and
/// attributes are synthesized from the statx mode + name.
pub fn stat_path(path: &str, is_dir_hint: bool) -> Option<IndexedFile> {
    let path_cstr = CString::new(path.as_bytes()).ok()?;
    let stat = statx_nofollow(&path_cstr).ok()?;

    let is_dir = if stat.stx_mode & S_IFMT == 0o040000 {
        true
    } else if stat.stx_mode & S_IFMT == S_IFREG {
        false
    } else {
        is_dir_hint
    };

    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let stx_mask = stat.stx_mask;
    let has_btime = (stx_mask & StatxFlags::BTIME.bits()) != 0;
    let created = if has_btime && stat.stx_btime.tv_sec != 0 {
        to_filetime(stat.stx_btime)
    } else if stat.stx_ctime.tv_sec != 0 {
        to_filetime(stat.stx_ctime)
    } else {
        to_filetime(stat.stx_mtime)
    };
    let modified = to_filetime(stat.stx_mtime);
    let accessed = to_filetime(stat.stx_atime);

    let mut file = IndexedFile::new(
        path.to_string(),
        if is_dir { 0 } else { stat.stx_size },
        created,
        modified,
        accessed,
        is_dir,
        stat.stx_ino,
    );
    file.attributes = synthesize_attributes(stat.stx_mode, is_dir, &name);
    Some(file)
}

/// Mount-aware volume identity: the mount root the path lives under.
///
/// Inodes are only unique per filesystem, so this must return the mount point
/// (e.g. "/" or "/home"), NOT a fixed "/", or index.rs's
/// (volume, file_ref) key will collide across filesystems.
pub fn volume_of(path: &str) -> String {
    volume_of_with(cached_mounts(), path)
}

/// Internal helper: longest-prefix mount match against a provided mount list.
/// Separated from the public `volume_of` so tests can inject an arbitrary
/// mount table without depending on the machine's actual mounts.
fn volume_of_with(mounts: &[String], path: &str) -> String {
    // Find the longest mount point that is a prefix of `path` at a
    // path-component boundary.
    let mut best: Option<&str> = None;
    for m in mounts {
        let mp = m.as_str();
        if path == mp || (path.starts_with(mp) && {
            let rest = &path[mp.len()..];
            rest.starts_with('/') || rest.is_empty()
        }) {
            match best {
                Some(prev) if prev.len() >= mp.len() => {}
                _ => best = Some(mp),
            }
        }
    }
    if let Some(m) = best {
        return m.to_string();
    }

    // No mount matched.  If path starts with "/" (but no mount matched),
    // return "/".  Otherwise (bare name), return the first component like
    // Windows does.
    if path.starts_with('/') {
        "/".to_string()
    } else {
        path.split('/').next().unwrap_or(path).to_string()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// statx with SYMLINK_NOFOLLOW (for entries where d_type already filtered
/// symlinks, but we want to be race-safe).
fn statx_nofollow(path: &CString) -> anyhow::Result<Statx> {
    Ok(rustix::fs::statx(CWD, path.as_c_str(), AtFlags::SYMLINK_NOFOLLOW, StatxFlags::BASIC_STATS | StatxFlags::BTIME)?)
}

/// statx following symlinks (used for the volume root directory).
fn statx_follow(dir: &Dir, name: &str) -> anyhow::Result<Statx> {
    let c = CString::new(name).unwrap();
    let fd = dir.fd().map_err(|e| anyhow::anyhow!(e))?;
    Ok(rustix::fs::statx(fd, c.as_c_str(), AtFlags::empty(), StatxFlags::BASIC_STATS | StatxFlags::BTIME)?)
}

// ===========================================================================
// Parse /proc/self/mounts
// ===========================================================================

/// Parse `/proc/self/mounts` and return deduplicated, sorted real-disk
/// mount points.
fn discover_volumes_inner() -> Vec<String> {
    match parse_proc_mounts() {
        Ok(v) if !v.is_empty() => v,
        _ => vec!["/".to_string()],
    }
}

fn parse_proc_mounts() -> Result<Vec<String>> {
    let content = std::fs::read_to_string("/proc/self/mounts")
        .context("read /proc/self/mounts")?;

    // Track best (longest) mount point per device id.
    // device id is (major << 32 | minor) from statfs or similar; we resolve
    // it by stat'ing each mount point.
    let mut best_by_dev: HashMap<u64, String> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Format: device mountpoint fstype options dump pass
        // Escaped spaces in paths are \040.
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        // Unescape \040 -> space in mount point.
        let mountpoint = parts[1].replace("\\040", " ");
        let fstype = parts[2];

        if !FSTYPE_WHITELIST.contains(&fstype) {
            continue;
        }

        // Get the device id by stat'ing the mount point.
        let dev = match std::fs::metadata(&mountpoint) {
            Ok(m) => {
                use std::os::unix::fs::MetadataExt;
                // Use raw dev() for consistency since we only compare
                // within this map.
                m.dev()
            }
            Err(_) => continue,
        };

        // Keep the longest (most specific) mount point per device.
        match best_by_dev.get(&dev) {
            Some(existing) if existing.len() >= mountpoint.len() => {}
            _ => {
                best_by_dev.insert(dev, mountpoint);
            }
        }
    }

    let mut result: Vec<String> = best_by_dev.into_values().collect();

    // Sort deterministically, but "/" always comes first.
    result.sort();
    result.retain(|p| p != "/");
    result.insert(0, "/".to_string());

    Ok(result)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_volumes_returns_mount_points() {
        let vols = discover_volumes();
        assert!(!vols.is_empty(), "discover_volumes should return at least /");
        for v in &vols {
            assert!(v.starts_with('/'), "mount point should start with /: {v}");
            assert!(
                std::path::Path::new(v).exists(),
                "mount point should exist on disk: {v}"
            );
        }
    }

    #[test]
    fn scan_volume_finds_known_files() {
        use std::fs;

        let tmp = std::env::temp_dir().join("instant_search_walk_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("subdir")).unwrap();
        fs::write(tmp.join("hello.txt"), b"hello world").unwrap();
        fs::write(tmp.join(".hidden"), b"secret").unwrap();

        let result = scan_volume(tmp.to_str().unwrap());
        assert!(result.is_ok(), "scan_volume failed: {:?}", result.err());
        let entries = result.unwrap();

        // We should have: hello.txt, .hidden, subdir (the temp dir itself
        // is not scanned, only its contents).
        assert!(
            entries.len() >= 3,
            "expected at least 3 entries, got {}",
            entries.len()
        );

        let hello = entries.iter().find(|e| e.name == "hello.txt").unwrap();
        assert!(!hello.is_dir);
        assert_eq!(hello.size, 11); // b"hello world".len()
        assert_eq!(hello.attributes & FILE_ATTRIBUTE_HIDDEN, 0, "hello.txt should not be hidden");
        assert_ne!(hello.attributes & FILE_ATTRIBUTE_ARCHIVE, 0, "hello.txt should have ARCHIVE");
        assert_ne!(hello.file_ref, 0, "inode should be non-zero");

        let hidden = entries.iter().find(|e| e.name == ".hidden").unwrap();
        assert!(!hidden.is_dir);
        assert_ne!(
            hidden.attributes & FILE_ATTRIBUTE_HIDDEN,
            0,
            ".hidden should have HIDDEN attribute"
        );

        let subdir = entries.iter().find(|e| e.name == "subdir").unwrap();
        assert!(subdir.is_dir);
        assert_ne!(
            subdir.attributes & FILE_ATTRIBUTE_DIRECTORY,
            0,
            "subdir should have DIRECTORY attribute"
        );

        // Clean up.
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn volume_of_longest_mount() {
        let mounts = vec!["/".to_string(), "/home".to_string()];
        assert_eq!(volume_of_with(&mounts, "/home/user/x"), "/home");
        assert_eq!(volume_of_with(&mounts, "/etc/passwd"), "/");
        assert_eq!(volume_of_with(&mounts, "/home"), "/home");
        assert_eq!(volume_of_with(&mounts, "/"), "/");
        // Bare name (no leading /).
        assert_eq!(volume_of_with(&mounts, "foo"), "foo");
        // No matching mount except "/".
        assert_eq!(volume_of_with(&mounts, "/opt/foo"), "/");
    }

    #[test]
    fn to_filetime_epoch() {
        // 1970-01-01 00:00:00 UTC maps to FILETIME epoch.
        // StatxTimestamp is #[non_exhaustive] in rustix, so we use
        // unsafe zeroed memory (all fields 0 is the trivial case).
        let ts = unsafe { std::mem::zeroed::<rustix::fs::StatxTimestamp>() };
        assert_eq!(to_filetime(ts), 0);
    }

    #[test]
    fn synthesize_attributes_hidden_dir() {
        let mode = 0o040755; // directory, rwxr-xr-x
        let attrs = synthesize_attributes(mode, true, ".hidden_dir");
        assert_ne!(attrs & FILE_ATTRIBUTE_DIRECTORY, 0);
        assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0);
        assert_eq!(attrs & FILE_ATTRIBUTE_READONLY, 0, "should not be readonly (has write bits)");
    }

    #[test]
    fn synthesize_attributes_readonly() {
        let mode = 0o100444; // regular file, r--r--r--
        let attrs = synthesize_attributes(mode, false, "readonly.txt");
        assert_ne!(attrs & FILE_ATTRIBUTE_READONLY, 0);
        assert_eq!(attrs & FILE_ATTRIBUTE_DIRECTORY, 0);
    }
}
