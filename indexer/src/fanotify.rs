//! Linux change-tracking backend for the instant-file-search indexer.
//!
//! Uses fanotify in FID mode (`FAN_REPORT_DFID_NAME | FAN_REPORT_TARGET_FID`)
//! to observe filesystem events without holding open file descriptors for
//! every watched object. One fanotify fd is marked on each volume root with
//! `FAN_MARK_FILESYSTEM`, so a single blocking read loop covers all volumes.
//!
//! Event semantics mirror the Windows USN watcher (`usn.rs`):
//!   - CREATE / MODIFY / ATTRIB / CLOSE_WRITE  -> "WRITE"   (re-stat + upsert)
//!   - DELETE                                  -> "DELETE"  (remove / remove_prefix)
//!   - MOVED_FROM / MOVED_TO                   -> "RENAME" / "RENAME_NEW"
//!     (paired by the moved object's file handle; directories re-prefix the
//!      whole subtree, since fanotify emits no per-child rename records)
//!
//! Requires root (or CAP_SYS_ADMIN) for `fanotify_init` with FID flags and
//! `FAN_MARK_FILESYSTEM`. There is intentionally no inotify fallback in this
//! backend yet: if fanotify cannot be initialized, the indexer reports a clear
//! startup error rather than claiming complete change tracking. On queue overflow (`FAN_Q_OVERFLOW`) the index is
//! rebuilt from a full re-scan via `index.replace(all)`, matching the USN
//! rollover recovery (volume-scoped replacement is unsafe on Linux: root "/"
//! is a prefix of every path).

use std::ffi::CString;
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::index::FileIndex;

// ---------------------------------------------------------------------------
// fanotify constants (libc exposes most; the info-type values match the
// kernel uapi headers).
// ---------------------------------------------------------------------------
const FAN_CLASS_NOTIF: u32 = 0x0000_0000;
const FAN_CLOEXEC: u32 = 0x0000_0001;
const FAN_UNLIMITED_QUEUE: u32 = 0x0000_0010;
const FAN_UNLIMITED_MARKS: u32 = 0x0000_0020;
const FAN_REPORT_FID: u32 = 0x0000_0200;
const FAN_REPORT_DIR_FID: u32 = 0x0000_0400;
const FAN_REPORT_NAME: u32 = 0x0000_0800;
const FAN_REPORT_TARGET_FID: u32 = 0x0000_1000;
const FAN_REPORT_DFID_NAME: u32 = FAN_REPORT_DIR_FID | FAN_REPORT_NAME;
const FAN_REPORT_DFID_NAME_TARGET: u32 =
    FAN_REPORT_DFID_NAME | FAN_REPORT_FID | FAN_REPORT_TARGET_FID;

const FAN_MARK_ADD: u32 = 0x0000_0001;
const FAN_MARK_FILESYSTEM: u32 = 0x0000_0100;

const FAN_MODIFY: u64 = 0x0000_0002;
const FAN_ATTRIB: u64 = 0x0000_0004;
const FAN_CLOSE_WRITE: u64 = 0x0000_0008;
const FAN_MOVED_FROM: u64 = 0x0000_0040;
const FAN_MOVED_TO: u64 = 0x0000_0080;
const FAN_CREATE: u64 = 0x0000_0100;
const FAN_DELETE: u64 = 0x0000_0200;
const FAN_Q_OVERFLOW: u64 = 0x0000_4000;
const FAN_ONDIR: u64 = 0x4000_0000;

const FAN_EVENT_INFO_TYPE_FID: u8 = 1;
const FAN_EVENT_INFO_TYPE_DFID_NAME: u8 = 2;
const FAN_EVENT_INFO_TYPE_DFID: u8 = 3;
const FAN_EVENT_INFO_TYPE_OLD_DFID_NAME: u8 = 10;
const FAN_EVENT_INFO_TYPE_NEW_DFID_NAME: u8 = 12;

// Windows FILETIME epoch offset: seconds between 1601-01-01 and 1970-01-01.
const FILETIME_EPOCH_OFFSET: i64 = 116_444_73600;

// ---------------------------------------------------------------------------
// Raw event structs (kernel uapi layout).
// ---------------------------------------------------------------------------
#[repr(C)]
struct FanotifyEventMetadata {
    event_len: u32,
    vers: u8,
    reserved: u8,
    metadata_len: u16,
    mask: u64,
    fd: i32,
    pid: i32,
}

#[repr(C)]
struct FanotifyEventInfoHeader {
    info_type: u8,
    pad: u8,
    len: u16,
}

/// A file handle from an event info record. The raw bytes double as the
/// rename-pairing key (identical across a MOVED_FROM / MOVED_TO pair).
#[derive(Clone)]
struct FileHandle {
    handle_bytes: u32,
    handle_type: i32,
    data: Vec<u8>,
}

/// A volume mount: its path and an O_PATH fd on the mount point, used as the
/// `mount_fd` argument to `open_by_handle_at`.
struct Mount {
    path: String,
    fd: RawFd,
}

// ---------------------------------------------------------------------------
// Public API (mirrors usn.rs)
// ---------------------------------------------------------------------------

/// Linux has no persistent change journal; return an empty tail list.
pub fn journal_tails(_volumes: &[String]) -> Vec<(String, u64, i64)> {
    Vec::new()
}

/// Watch all volumes for changes via a single fanotify fd, applying events to
/// the index and content store until the process exits.
pub fn watch_all(
    volumes: &[String],
    index: &Arc<FileIndex>,
    content: &Arc<crate::content::ContentStore>,
    _tails: &[(String, u64, i64)],
) -> Result<()> {
    let fd = fanotify_init()?;

    // Open an O_PATH fd on each volume root for handle resolution.
    let mut mounts: Vec<Mount> = Vec::new();
    for v in volumes {
        let c = CString::new(v.as_str()).context("volume path contains NUL")?;
        let mfd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if mfd < 0 {
            tracing::warn!(
                "fanotify: cannot open mount {}: {}",
                v,
                std::io::Error::last_os_error()
            );
            continue;
        }
        mounts.push(Mount { path: v.clone(), fd: mfd });
    }
    if mounts.is_empty() {
        unsafe { libc::close(fd) };
        anyhow::bail!("fanotify: no volumes could be opened for watching");
    }

    // Mark each volume filesystem-wide.
    let mark_mask = FAN_CREATE
        | FAN_DELETE
        | FAN_MOVED_FROM
        | FAN_MOVED_TO
        | FAN_MODIFY
        | FAN_ATTRIB
        | FAN_CLOSE_WRITE;
    let mut marked = 0usize;
    for m in &mounts {
        let c = CString::new(m.path.as_str()).map_err(|_| anyhow::anyhow!("NUL in mount path"))?;
        let rc = unsafe {
            libc::fanotify_mark(
                fd,
                FAN_MARK_ADD | FAN_MARK_FILESYSTEM,
                mark_mask,
                libc::AT_FDCWD,
                c.as_ptr(),
            )
        };
        if rc < 0 {
            tracing::warn!(
                "fanotify: mark {} failed: {}",
                m.path,
                std::io::Error::last_os_error()
            );
        } else {
            marked += 1;
            tracing::info!("fanotify: watching {} (filesystem-wide)", m.path);
        }
    }
    if marked == 0 {
        for m in &mounts {
            unsafe { libc::close(m.fd) };
        }
        unsafe { libc::close(fd) };
        anyhow::bail!("fanotify: no volume marks succeeded");
    }

    tracing::info!("fanotify: change watcher started on {marked} volume(s)");
    let result = event_loop(fd, &mounts, volumes, index, content);
    for m in &mounts {
        unsafe { libc::close(m.fd) };
    }
    unsafe { libc::close(fd) };
    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

fn event_loop(
    fd: RawFd,
    mounts: &[Mount],
    volumes: &[String],
    index: &Arc<FileIndex>,
    content: &Arc<crate::content::ContentStore>,
) -> Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    // Pending renames keyed by the moved object's file-handle bytes.
    let mut pending: std::collections::HashMap<Vec<u8>, String> = std::collections::HashMap::new();

    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e).context("fanotify read");
        }
        if n == 0 {
            continue;
        }
        let n = n as usize;

        let mut off = 0usize;
        while off + std::mem::size_of::<FanotifyEventMetadata>() <= n {
            let meta = unsafe { &*(buf.as_ptr().add(off) as *const FanotifyEventMetadata) };
            if meta.event_len == 0 || meta.event_len as usize > n - off {
                break;
            }
            let event_end = off + meta.event_len as usize;

            if meta.mask & FAN_Q_OVERFLOW != 0 {
                tracing::warn!("fanotify: queue overflow; rebuilding index");
                if let Err(e) = crate::scan::build_index(volumes, index) {
                    tracing::warn!("fanotify: overflow rescan failed: {e:#}");
                }
                off = event_end;
                continue;
            }

            // Parse info records (DFID_NAME / DFID / FID).
            let mut parent_handle: Option<FileHandle> = None;
            let mut name: Option<String> = None;
            let mut target_handle: Option<FileHandle> = None;
            let mut info_off = off + meta.metadata_len as usize;
            while info_off + std::mem::size_of::<FanotifyEventInfoHeader>() <= event_end {
                let hdr = unsafe { &*(buf.as_ptr().add(info_off) as *const FanotifyEventInfoHeader) };
                let hlen = hdr.len as usize;
                if hlen < std::mem::size_of::<FanotifyEventInfoHeader>()
                    || info_off + hlen > event_end
                {
                    break;
                }
                // Info record layout: header(4) | fsid(8) | file_handle(8+bytes).
                let handle_start = info_off + 4 + 8;
                let handle = parse_handle(&buf, handle_start, event_end);
                match hdr.info_type {
                    FAN_EVENT_INFO_TYPE_DFID_NAME | FAN_EVENT_INFO_TYPE_NEW_DFID_NAME => {
                        parent_handle = handle.clone();
                        // The entry name follows the parent file handle.
                        if let Some(h) = &parent_handle {
                            let name_start = handle_start + 8 + h.handle_bytes as usize;
                            if name_start < event_end {
                                let name_bytes = &buf[name_start..event_end];
                                let end = name_bytes
                                    .iter()
                                    .position(|&b| b == 0)
                                    .unwrap_or(name_bytes.len());
                                name = Some(
                                    String::from_utf8_lossy(&name_bytes[..end]).into_owned(),
                                );
                            }
                        }
                    }
                    FAN_EVENT_INFO_TYPE_FID => {
                        // The moved object's own handle (TARGET_FID).
                        target_handle = handle;
                    }
                    FAN_EVENT_INFO_TYPE_DFID | FAN_EVENT_INFO_TYPE_OLD_DFID_NAME => {
                        parent_handle = handle;
                    }
                    _ => {}
                }
                info_off += hlen;
            }

            let is_dir = meta.mask & FAN_ONDIR != 0;
            let path = resolve_path(mounts, &parent_handle, name.as_deref())
                .or_else(|| resolve_handle_path(mounts, &target_handle));

            if let Some(path) = path {
                apply_event(index, content, meta.mask, is_dir, &path, &target_handle, &mut pending);
            } else {
                tracing::debug!("fanotify: could not resolve path for mask=0x{:X}", meta.mask);
            }

            off = event_end;
        }
    }
}

/// Parse a `file_handle` (u32 handle_bytes, i32 handle_type, bytes) from a
/// buffer slice, returning None if the slice is too short.
fn parse_handle(buf: &[u8], start: usize, limit: usize) -> Option<FileHandle> {
    if start + 8 > limit {
        return None;
    }
    let handle_bytes = u32::from_le_bytes(buf[start..start + 4].try_into().ok()?);
    let handle_type = i32::from_le_bytes(buf[start + 4..start + 8].try_into().ok()?);
    let data_start = start + 8;
    let data_end = data_start + handle_bytes as usize;
    if data_end > limit {
        return None;
    }
    Some(FileHandle {
        handle_bytes,
        handle_type,
        data: buf[data_start..data_end].to_vec(),
    })
}

/// Resolve a parent-dir handle + entry name to a full path.
fn resolve_path(
    mounts: &[Mount],
    parent: &Option<FileHandle>,
    name: Option<&str>,
) -> Option<String> {
    let parent_path = resolve_handle_path(mounts, parent)?;
    match name {
        Some(n) if !n.is_empty() => Some(format!("{}/{}", parent_path.trim_end_matches('/'), n)),
        _ => Some(parent_path),
    }
}

/// Resolve a file handle to a path by trying `open_by_handle_at` on each
/// volume's mount fd, then reading `/proc/self/fd/N`.
fn resolve_handle_path(mounts: &[Mount], handle: &Option<FileHandle>) -> Option<String> {
    let h = handle.as_ref()?;
    // Build a raw file_handle buffer: [u32 handle_bytes][i32 handle_type][data].
    let mut raw = Vec::with_capacity(8 + h.data.len());
    raw.extend_from_slice(&h.handle_bytes.to_le_bytes());
    raw.extend_from_slice(&h.handle_type.to_le_bytes());
    raw.extend_from_slice(&h.data);

    for m in mounts {
        let fd = unsafe {
            libc::open_by_handle_at(m.fd, raw.as_ptr() as *mut libc::file_handle, libc::O_PATH)
        };
        if fd < 0 {
            continue; // handle not on this mount
        }
        let path = readlink_fd(fd);
        unsafe { libc::close(fd) };
        if let Some(p) = path {
            return Some(p);
        }
    }
    None
}

fn readlink_fd(fd: RawFd) -> Option<String> {
    let link = format!("/proc/self/fd/{fd}");
    let c = CString::new(link).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::readlink(c.as_ptr(), buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if n < 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// Apply a single fanotify event to the index and content store, mirroring
/// the USN watcher's reason handling.
fn apply_event(
    index: &Arc<FileIndex>,
    content: &Arc<crate::content::ContentStore>,
    mask: u64,
    is_dir: bool,
    path: &str,
    target_handle: &Option<FileHandle>,
    pending: &mut std::collections::HashMap<Vec<u8>, String>,
) {
    let now = now_filetime();

    if mask & FAN_DELETE != 0 {
        if is_dir {
            index.remove_prefix(path);
        } else {
            index.remove(path);
        }
        index.record_change(now, "DELETE", path, is_dir);
        content.remove(path);
        tracing::debug!("fanotify: DELETE {}", path);
    } else if mask & FAN_MOVED_FROM != 0 {
        // Remember the old path keyed by the moved object's handle so the
        // matching MOVED_TO can re-prefix the subtree.
        if let Some(h) = target_handle {
            pending.insert(h.data.clone(), path.to_string());
        }
        if !is_dir {
            index.remove(path);
        }
        index.record_change(now, "RENAME", path, is_dir);
        content.remove(path);
        tracing::debug!("fanotify: RENAME_OLD {}", path);
    } else if mask & FAN_MOVED_TO != 0 {
        if let Some(h) = target_handle {
            if let Some(old_path) = pending.remove(&h.data) {
                if is_dir && old_path != path {
                    index.rename_prefix(&old_path, path);
                    tracing::info!("fanotify: RENAME dir {} -> {}", old_path, path);
                }
            }
        }
        index.record_change(now, "RENAME_NEW", path, is_dir);
        content.remove(path);
        upsert_or_remove(index, content, path, is_dir);
    } else if mask & (FAN_CREATE | FAN_MODIFY | FAN_ATTRIB | FAN_CLOSE_WRITE) != 0 {
        index.record_change(now, "WRITE", path, is_dir);
        upsert_or_remove(index, content, path, is_dir);
    }
}

/// Stat a path and upsert it into the index (and content store if eligible).
/// If the file is gone, remove it instead.
fn upsert_or_remove(
    index: &Arc<FileIndex>,
    content: &Arc<crate::content::ContentStore>,
    path: &str,
    is_dir: bool,
) {
    if let Some(entry) = crate::walk::stat_path(path, is_dir) {
        if !entry.is_dir && crate::content::ContentStore::should_index(path, entry.size) {
            if let Ok(data) = std::fs::read(path) {
                let keep = data.len().min(crate::content::MAX_FILE_BYTES as usize);
                content.insert(path, &data[..keep]);
            }
        } else {
            content.remove(path);
        }
        index.upsert(entry);
    } else {
        index.remove(path);
        content.remove(path);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fanotify_init() -> Result<RawFd> {
    let flags = FAN_CLASS_NOTIF
        | FAN_REPORT_DFID_NAME_TARGET
        | FAN_CLOEXEC
        | FAN_UNLIMITED_QUEUE
        | FAN_UNLIMITED_MARKS;
    let fd = unsafe { libc::fanotify_init(flags, libc::O_RDONLY as u32) };
    if fd < 0 {
        let first_err = std::io::Error::last_os_error();
        // Retry without the CAP_SYS_ADMIN-gated unlimited flags.
        let flags2 = FAN_CLASS_NOTIF | FAN_REPORT_DFID_NAME_TARGET | FAN_CLOEXEC;
        let fd2 = unsafe { libc::fanotify_init(flags2, libc::O_RDONLY as u32) };
        if fd2 < 0 {
            return Err(first_err).context("fanotify_init (FID mode requires CAP_SYS_ADMIN)");
        }
        return Ok(fd2);
    }
    Ok(fd)
}

fn now_filetime() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64 + FILETIME_EPOCH_OFFSET) * 10_000_000
            + (d.subsec_nanos() as i64 / 100),
        Err(e) => (e.duration().as_secs() as i64 + FILETIME_EPOCH_OFFSET) * 10_000_000,
    }
}
