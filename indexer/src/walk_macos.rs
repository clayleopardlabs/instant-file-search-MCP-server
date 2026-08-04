//! macOS enumeration backend for the instant-file-search indexer.
//!
//! Uses `getattrlistbulk` (the batched statx-equivalent) to walk directory
//! trees, building `IndexedFile` entries with APFS fileid as `file_ref`,
//! creation time from `ATTR_CMN_CRTIME`, and attributes synthesized from the
//! vnode type, UF_* flags, and file name.
//!
//! Volume identity uses the REAL device id (`FSOPT_RETURN_REALDEV`), which
//! resolves the System/Data volume-group trap: the sealed system volume and
//! the Data volume share one logical device, and firmlinks at /Users,
//! /Applications, /Library, /private/var, /opt redirect into Data. Walking
//! "/" therefore covers both system files and user data through firmlinks;
//! the /System/Volumes/* internal mounts are skipped to avoid double-counting
//! Data content.

use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};

use crate::types::IndexedFile;

// ---------------------------------------------------------------------------
// FILE_ATTRIBUTE_* constants (Windows semantics, synthesized on macOS).
// Matches the `attrib:` query filter in query.rs.
// ---------------------------------------------------------------------------
const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;

// Windows FILETIME epoch offset: seconds between 1601-01-01 and 1970-01-01.
const FILETIME_EPOCH_OFFSET: i64 = 116_444_73600;

// vnode types (from <sys/vnode.h>; libc does not expose them).
const VREG: u32 = 1; // regular file
const VDIR: u32 = 2; // directory
const VLNK: u32 = 5; // symbolic link

// UF_HIDDEN flag on the ATTR_CMN_FLAGS value.
const UF_HIDDEN: u32 = 0x0000_8000;

// ATTR_CMN_ERROR (libc 0.2.186 does not expose it; added in 0.2.189).
const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;

/// Filesystem types considered "real disk" volumes.
/// Pseudo/virtual/virtualised types are excluded.
const FSTYPE_WHITELIST: &[&str] = &[
    "apfs", "hfs", "hfs+", "exfat", "msdos", "ntfs",
];

// ---------------------------------------------------------------------------
// Attribute list: a single shared request for getattrlist / getattrlistbulk.
//
// The record layout is: u32 total length, then the RETURNED_ATTRS
// attribute_set_t (20 bytes), then each requested attribute in fixed order,
// each gated on the returned bit (zero-filled under FSOPT_PACK_INVAL_ATTRS).
// ---------------------------------------------------------------------------
const ATTR_CMN_DEVID: libc::attrgroup_t = 0x0000_0002;
const ATTR_CMN_OBJTYPE: libc::attrgroup_t = 0x0000_0008;
const ATTR_CMN_CRTIME: libc::attrgroup_t = 0x0000_0200;
const ATTR_CMN_MODTIME: libc::attrgroup_t = 0x0000_0400;
const ATTR_CMN_ACCTIME: libc::attrgroup_t = 0x0000_1000;
const ATTR_CMN_FLAGS: libc::attrgroup_t = 0x0004_0000;
const ATTR_CMN_FILEID: libc::attrgroup_t = 0x0200_0000;
const ATTR_CMN_RETURNED_ATTRS: libc::attrgroup_t = 0x8000_0000;

const ATTR_FILE_DATALENGTH: libc::attrgroup_t = 0x0000_0200;

const ATTRLIST_COMMON: libc::attrgroup_t = ATTR_CMN_RETURNED_ATTRS
    | ATTR_CMN_ERROR
    | ATTR_CMN_DEVID
    | ATTR_CMN_OBJTYPE
    | ATTR_CMN_CRTIME
    | ATTR_CMN_MODTIME
    | ATTR_CMN_ACCTIME
    | ATTR_CMN_FLAGS
    | ATTR_CMN_FILEID;

fn make_attrlist() -> libc::attrlist {
    libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTRLIST_COMMON,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_DATALENGTH,
        forkattr: 0,
    }
}

/// Bulk buffer size: large enough for ~256 entries per call.
const BULK_BUF: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// C string helpers
// ---------------------------------------------------------------------------
fn cstr_from_fixed(arr: &[c_char]) -> String {
    let bytes: Vec<u8> = arr
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Record parsing
// ---------------------------------------------------------------------------

/// One parsed getattrlistbulk record.
struct Entry {
    name: String,
    devid: u32,
    objtype: u32,
    crtime: i64, // FILETIME (100ns since 1601), 0 if absent
    modtime: i64,
    acctime: i64,
    flags: u32,
    fileid: u64,
    datalength: u64,
    error: u32,
}

/// Convert a macOS timespec (Unix epoch seconds) to FILETIME.
fn to_filetime(sec: i64, nsec: i64) -> i64 {
    if sec == 0 {
        return 0;
    }
    (sec + FILETIME_EPOCH_OFFSET) * 10_000_000 + nsec / 100
}

/// Read a timespec (16 bytes: tv_sec u64, tv_nsec u64) at `off`.
fn read_timespec(buf: &[u8], off: usize) -> (i64, i64) {
    let sec = u64::from_le_bytes(
        buf[off..off + 8].try_into().expect("timespec sec"),
    ) as i64;
    let nsec = u64::from_le_bytes(
        buf[off + 8..off + 16].try_into().expect("timespec nsec"),
    ) as i64;
    (sec, nsec)
}

/// Parse one bulk record starting at `off`. Returns the entry and the next
/// record offset. The record layout (per getattrlistbulk(2)) is:
///
///   u32 length  (whole record, 8-aligned)
///   attribute_set_t returned  (20 bytes: common/vol/dir/file/fork attrs)
///   ATTR_CMN_ERROR (u32)            — if requested and returned
///   ATTR_CMN_NAME (attrreference)   — if requested and returned
///   ATTR_CMN_DEVID (u32)            — real devid under FSOPT_RETURN_REALDEV
///   ATTR_CMN_OBJTYPE (u32)
///   ATTR_CMN_CRTIME (timespec, 16)
///   ATTR_CMN_MODTIME (timespec, 16)
///   ATTR_CMN_ACCTIME (timespec, 16)
///   ATTR_CMN_FLAGS (u32)
///   ATTR_CMN_FILEID (u64, 4-aligned)
///   ATTR_FILE_DATALENGTH (u64, 4-aligned)
///   name string (NUL-terminated, 4-padded) — at 24 + attr_dataoffset
fn parse_record(buf: &[u8], off: usize) -> Option<(Entry, usize)> {
    if off + 24 > buf.len() {
        return None;
    }
    let len = u32::from_le_bytes(buf[off..off + 4].try_into().ok()?) as usize;
    if len < 24 || off + len > buf.len() {
        return None;
    }
    let rec = &buf[off..off + len];

    // returned attribute_set_t at offset 4.
    let returned_common = u32::from_le_bytes(rec[4..8].try_into().ok()?);
    let returned_file = u32::from_le_bytes(rec[16..20].try_into().ok()?);

    let mut e = Entry {
        name: String::new(),
        devid: 0,
        objtype: 0,
        crtime: 0,
        modtime: 0,
        acctime: 0,
        flags: 0,
        fileid: 0,
        datalength: 0,
        error: 0,
    };

    // Walk the fixed order, advancing only past attributes whose returned
    // bit is set (absent attributes are omitted from the record).
    let mut p = 24usize;

    if returned_common & ATTR_CMN_ERROR != 0 {
        e.error = u32::from_le_bytes(rec[p..p + 4].try_into().ok()?);
        p += 4;
    }
    // ATTR_CMN_NAME (bit 0x1): an attrreference (attr_dataoffset i32,
    // attr_length u32). The name string lives at `p + attr_dataoffset`.
    if returned_common & 0x1 != 0 {
        let dataoffset = i32::from_le_bytes(rec[p..p + 4].try_into().ok()?);
        let _namelen = u32::from_le_bytes(rec[p + 4..p + 8].try_into().ok()?);
        let str_off = (p as i64 + dataoffset as i64) as usize;
        if str_off < rec.len() {
            let slice = &rec[str_off..];
            let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
            e.name = String::from_utf8_lossy(&slice[..end]).into_owned();
        }
        p += 8;
    }
    if returned_common & ATTR_CMN_DEVID != 0 {
        e.devid = u32::from_le_bytes(rec[p..p + 4].try_into().ok()?);
        p += 4;
    }
    if returned_common & ATTR_CMN_OBJTYPE != 0 {
        e.objtype = u32::from_le_bytes(rec[p..p + 4].try_into().ok()?);
        p += 4;
    }
    if returned_common & ATTR_CMN_CRTIME != 0 {
        let (s, n) = read_timespec(rec, p);
        e.crtime = to_filetime(s, n);
        p += 16;
    }
    if returned_common & ATTR_CMN_MODTIME != 0 {
        let (s, n) = read_timespec(rec, p);
        e.modtime = to_filetime(s, n);
        p += 16;
    }
    if returned_common & ATTR_CMN_ACCTIME != 0 {
        let (s, n) = read_timespec(rec, p);
        e.acctime = to_filetime(s, n);
        p += 16;
    }
    if returned_common & ATTR_CMN_FLAGS != 0 {
        e.flags = u32::from_le_bytes(rec[p..p + 4].try_into().ok()?);
        p += 4;
    }
    if returned_common & ATTR_CMN_FILEID != 0 {
        e.fileid = u64::from_le_bytes(rec[p..p + 8].try_into().ok()?);
        p += 8;
    }
    if returned_file & ATTR_FILE_DATALENGTH != 0 {
        e.datalength = u64::from_le_bytes(rec[p..p + 8].try_into().ok()?);
        p += 8;
    }

    Some((e, off + len))
}

// ---------------------------------------------------------------------------
// Attribute synthesis
// ---------------------------------------------------------------------------

/// Derive Windows FILE_ATTRIBUTE_* flags from the vnode type, UF_* flags,
/// and file name.
fn synthesize_attributes(objtype: u32, flags: u32, is_dir: bool, name: &str) -> u32 {
    let mut attrs = 0u32;
    if is_dir {
        attrs |= FILE_ATTRIBUTE_DIRECTORY;
    }
    if name.starts_with('.') || (flags & UF_HIDDEN) != 0 {
        attrs |= FILE_ATTRIBUTE_HIDDEN;
    }
    if objtype == VREG {
        // ARCHIVE is the Windows default for regular files.
        attrs |= FILE_ATTRIBUTE_ARCHIVE;
    }
    // READONLY is not derivable from getattrlist without ATTR_CMN_ACCESSMASK;
    // we do not request permission bits, so this stays unset (matches the
    // Linux walker's conservative default of only setting it from mode bits).
    attrs
}

// ---------------------------------------------------------------------------
// Directory iteration
// ---------------------------------------------------------------------------

/// Open a directory for iteration. Symlinks are not followed; if the path is
/// a firmlink (which macOS exposes as a symlink at the VFS level for
/// /System/Volumes/Data and friends), retry without O_NOFOLLOW.
fn open_dir(path: &str) -> Result<OwnedFd> {
    let c = CString::new(path).context("path contains NUL")?;
    let mut flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    let mut fd = unsafe { libc::open(c.as_ptr(), flags) };
    if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ELOOP) {
        // Firmlink: retry following the link.
        flags &= !libc::O_NOFOLLOW;
        fd = unsafe { libc::open(c.as_ptr(), flags) };
    }
    if fd < 0 {
        bail!("open directory {}: {}", path, std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The real device id of a directory fd, using FSOPT_RETURN_REALDEV.
fn real_devid_of_fd(fd: &OwnedFd) -> Result<u32> {
    let mut al = make_attrlist();
    al.commonattr = ATTR_CMN_DEVID;
    al.fileattr = 0;
    let mut buf = [0u8; 8];
    let r = unsafe {
        libc::fgetattrlist(
            fd.as_raw_fd(),
            &mut al as *mut libc::attrlist as *mut libc::c_void,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            libc::FSOPT_RETURN_REALDEV,
        )
    };
    if r < 0 {
        bail!("fgetattrlist devid: {}", std::io::Error::last_os_error());
    }
    // Single-file output: u32 length + attrs (same order). DEVID is a u32 at
    // offset 4 (after the length prefix) with commonattr=DEVID only.
    Ok(u32::from_le_bytes(buf[4..8].try_into().expect("devid")))
}

// ===========================================================================
// Public API
// ===========================================================================

/// Real-disk mount points, deepest-first, deduplicated.
///
/// Uses `getmntinfo`, filters by fstype whitelist, skips the internal
/// /System/Volumes/* mounts (Data content is reached through firmlinks from
/// "/"), deduplicates by fsid, and returns them with "/" first.
pub fn discover_volumes() -> Vec<String> {
    cached_mounts().clone()
}

fn cached_mounts() -> &'static Vec<String> {
    static MOUNTS: OnceLock<Vec<String>> = OnceLock::new();
    MOUNTS.get_or_init(discover_volumes_inner)
}

fn discover_volumes_inner() -> Vec<String> {
    let mut buf: *mut libc::statfs = std::ptr::null_mut();
    let n = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if n <= 0 {
        return vec!["/".to_string()];
    }
    let mounts = unsafe { std::slice::from_raw_parts(buf, n as usize) };

    let mut best_by_dev: HashMap<u64, String> = HashMap::new();

    for m in mounts {
        let fstype = cstr_from_fixed(&m.f_fstypename);
        if !FSTYPE_WHITELIST.contains(&fstype.as_str()) {
            continue;
        }
        let mountpoint = cstr_from_fixed(&m.f_mntonname);
        if mountpoint.is_empty() {
            continue;
        }
        // Skip internal System/Data volume-group mounts: their content is
        // reached through firmlinks from "/", so indexing them separately
        // would double-count everything.
        if mountpoint.starts_with("/System/Volumes/") {
            continue;
        }

        // Device identity: fsid (two i32s) combined into one u64.
        let dev: u64 = {
            let raw: [i32; 2] = unsafe { std::mem::transmute(m.f_fsid) };
            ((raw[0] as u32 as u64) << 32) | (raw[1] as u32 as u64)
        };

        match best_by_dev.get(&dev) {
            Some(existing) if existing.len() >= mountpoint.len() => {}
            _ => {
                best_by_dev.insert(dev, mountpoint);
            }
        }
    }

    let mut result: Vec<String> = best_by_dev.into_values().collect();
    result.sort();
    result.retain(|p| p != "/");
    result.insert(0, "/".to_string());
    result
}

/// Walk one volume recursively, returning every file and directory as an
/// IndexedFile.
///
/// Does NOT descend into other volumes (cross-device guard by real devid)
/// and does NOT follow symlinks.
pub fn scan_volume(volume: &str) -> Result<Vec<IndexedFile>> {
    let root_fd = open_dir(volume)?;
    let root_dev = real_devid_of_fd(&root_fd)?;

    let mut entries = Vec::new();
    let mut stack: Vec<(PathBuf, OwnedFd)> = vec![(PathBuf::from(volume), root_fd)];

    while let Some((dir_path, fd)) = stack.pop() {
        let mut al = make_attrlist();
        let mut buf = vec![0u8; BULK_BUF];

        loop {
            let n = unsafe {
                libc::getattrlistbulk(
                    fd.as_raw_fd(),
                    &mut al as *mut libc::attrlist as *mut libc::c_void,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    libc::FSOPT_RETURN_REALDEV as u64,
                )
            };
            if n <= 0 {
                break;
            }
            let mut off = 0usize;
            for _ in 0..n {
                let Some((entry, next)) = parse_record(&buf, off) else {
                    break;
                };
                off = next;

                if entry.error != 0 || entry.name.is_empty() || entry.name == "." || entry.name == ".." {
                    continue;
                }

                // Cross-device guard: skip entries on another volume.
                if entry.devid != root_dev {
                    continue;
                }

                let is_dir = entry.objtype == VDIR;

                // Skip symlinks entirely (matches Windows/Linux behavior).
                if entry.objtype == VLNK {
                    continue;
                }

                let full = format!("{}/{}", dir_path.display(), entry.name);

                // Never descend into the System volume-group internals.
                if full.starts_with("/System/Volumes/") {
                    continue;
                }

                let mut f = IndexedFile::new(
                    full.clone(),
                    if is_dir { 0 } else { entry.datalength },
                    entry.crtime,
                    entry.modtime,
                    entry.acctime,
                    is_dir,
                    entry.fileid,
                );
                f.attributes = synthesize_attributes(entry.objtype, entry.flags, is_dir, &entry.name);
                entries.push(f);

                if is_dir {
                    match open_dir(&full) {
                        Ok(child) => stack.push((PathBuf::from(full), child)),
                        Err(e) => {
                            tracing::debug!("skip unreadable dir {full}: {e:#}");
                        }
                    }
                }
            }
        }
    }

    Ok(entries)
}

/// Stat a single path and return its IndexedFile, for incremental updates
/// from the FSEvents watcher. Returns None if the path is gone or unreadable.
pub fn stat_path(path: &str, is_dir_hint: bool) -> Option<IndexedFile> {
    let c = CString::new(path).ok()?;
    let mut al = make_attrlist();
    let mut buf = [0u8; 512];
    let r = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            &mut al as *mut libc::attrlist as *mut libc::c_void,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            libc::FSOPT_RETURN_REALDEV,
        )
    };
    if r < 0 {
        return None;
    }
    let r = r as usize;
    if r > buf.len() {
        return None;
    }
    let (entry, _) = parse_record(&buf, 0)?;
    if entry.error != 0 {
        return None;
    }

    let is_dir = if entry.objtype == VDIR {
        true
    } else if entry.objtype == VREG {
        false
    } else {
        is_dir_hint
    };

    let mut f = IndexedFile::new(
        path.to_string(),
        if is_dir { 0 } else { entry.datalength },
        entry.crtime,
        entry.modtime,
        entry.acctime,
        is_dir,
        entry.fileid,
    );
    f.attributes = synthesize_attributes(entry.objtype, entry.flags, is_dir, &f.name);
    Some(f)
}

/// Mount-aware volume identity: the longest mount point prefix of `path`.
pub fn volume_of(path: &str) -> String {
    volume_of_with(cached_mounts(), path)
}

fn volume_of_with(mounts: &[String], path: &str) -> String {
    let mut best: Option<&str> = None;
    for m in mounts {
        if path == m || (path.starts_with(m) && path.as_bytes().get(m.len()) == Some(&b'/')) {
            if best.map_or(true, |b| m.len() > b.len()) {
                best = Some(m);
            }
        }
    }
    best.unwrap_or("/").to_string()
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_of_longest_mount_match() {
        let mounts = vec!["/".to_string(), "/Volumes/Ext".to_string()];
        assert_eq!(volume_of_with(&mounts, "/Users/alice/a.txt"), "/");
        assert_eq!(volume_of_with(&mounts, "/Volumes/Ext/x/y"), "/Volumes/Ext");
        // Mount prefix must be a path component, not a partial string.
        assert_eq!(volume_of_with(&mounts, "/Volumes/Extra"), "/");
        assert_eq!(volume_of_with(&mounts, "/"), "/");
    }

    #[test]
    fn parse_record_handles_minimal_record() {
        // Hand-build a minimal record: length + returned set with only
        // RETURNED_ATTRS bit set (nothing else returned).
        let mut buf = vec![0u8; 24];
        let len = 24u32;
        buf[0..4].copy_from_slice(&len.to_le_bytes());
        buf[4..8].copy_from_slice(&ATTR_CMN_RETURNED_ATTRS.to_le_bytes());
        let (e, next) = parse_record(&buf, 0).unwrap();
        assert_eq!(next, 24);
        assert!(e.name.is_empty());
        assert_eq!(e.error, 0);
    }

    #[test]
    fn synthesize_hidden_from_dot_and_uf_hidden() {
        let attrs = synthesize_attributes(VREG, 0, false, ".hidden");
        assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0);
        let attrs = synthesize_attributes(VREG, UF_HIDDEN, false, "visible");
        assert_ne!(attrs & FILE_ATTRIBUTE_HIDDEN, 0);
        let attrs = synthesize_attributes(VREG, 0, false, "plain.txt");
        assert_eq!(attrs & FILE_ATTRIBUTE_HIDDEN, 0);
        assert_ne!(attrs & FILE_ATTRIBUTE_ARCHIVE, 0);
    }

    #[test]
    fn to_filetime_epoch_matches() {
        // 1970-01-01T00:00:00Z -> 116444736000000000 FILETIME.
        assert_eq!(to_filetime(0, 0), 0);
        assert_eq!(to_filetime(116_444_73600, 0), 116_444_73600 * 10_000_000);
    }
}
