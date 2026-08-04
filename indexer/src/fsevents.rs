//! macOS change tracking via FSEvents.
//!
//! One `FSEventStream` per volume with `kFSEventStreamCreateFlagFileEvents` +
//! `kFSEventStreamCreateFlagUseExtendedData`; `since_when` is the volume's
//! journal tail captured before the initial scan, so events that land during
//! the scan window are replayed once watching starts (FSEvents keeps a
//! persistent per-device journal, unlike Linux fanotify).
//!
//! Extended data provides a per-event `fileID` (the APFS fileid, identical to
//! `walk_macos`'s `file_ref`), which pairs renames through
//! `index::path_by_ref(volume, fileid)`. Run-loop dispatch is used
//! (`FSEventStreamScheduleWithRunLoop` + `CFRunLoopRun` blocks the watcher
//! thread); this avoids the `dispatch2` dependency.
//!
//! Overflow flags (`MustScanSubDirs`, `UserDropped`, `KernelDropped`,
//! `EventIdsWrapped`, `RootChanged`) trigger a full re-scan via
//! `scan::build_index`, mirroring fanotify's queue-overflow recovery.

use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRunLoop, CFString, CFType, CFRetained,
    kCFRunLoopDefaultMode,
};
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamCreateFlags,
    FSEventStreamEventFlags, FSEventStreamEventId, FSEventStreamRef, FSEventStreamInvalidate,
    FSEventStreamScheduleWithRunLoop, FSEventStreamStart, FSEventsGetLastEventIdForDeviceBeforeTime,
    kFSEventStreamCreateFlagFileEvents, kFSEventStreamCreateFlagIgnoreSelf,
    kFSEventStreamCreateFlagNoDefer, kFSEventStreamCreateFlagUseExtendedData,
    kFSEventStreamCreateFlagWatchRoot, kFSEventStreamEventFlagEventIdsWrapped,
    kFSEventStreamEventFlagItemChangeOwner, kFSEventStreamEventFlagItemCloned,
    kFSEventStreamEventFlagItemCreated, kFSEventStreamEventFlagItemFinderInfoMod,
    kFSEventStreamEventFlagItemInodeMetaMod, kFSEventStreamEventFlagItemIsDir,
    kFSEventStreamEventFlagItemModified, kFSEventStreamEventFlagItemRemoved,
    kFSEventStreamEventFlagItemRenamed, kFSEventStreamEventFlagItemXattrMod,
    kFSEventStreamEventFlagKernelDropped, kFSEventStreamEventFlagMustScanSubDirs,
    kFSEventStreamEventFlagRootChanged, kFSEventStreamEventFlagUserDropped,
    kFSEventStreamEventIdSinceNow,
};

use crate::content::ContentStore;
use crate::index::FileIndex;

/// Extended-data dictionary keys (FSEvents.h `#define`s, not in objc2 bindings).
const EXT_DATA_PATH: &str = "path";
const EXT_DATA_FILE_ID: &str = "fileID";

const FILETIME_EPOCH_OFFSET: i64 = 116_444_73600;

/// Shared state handed to the FSEvents callback via the stream context.
struct WatcherState {
    index: Arc<FileIndex>,
    content: Arc<ContentStore>,
    volumes: Vec<String>,
    path_key: CFRetained<CFString>,
    fileid_key: CFRetained<CFString>,
}

/// FSEvents keeps a persistent per-device journal, so we can return real tails:
/// the last event id before now for each volume. `since_when` = that id replays
/// events that landed during the initial scan window.
pub fn journal_tails(volumes: &[String]) -> Vec<(String, u64, i64)> {
    let mut tails = Vec::new();
    for v in volumes {
        let id = last_event_id_before_now(v);
        tracing::info!("fsevents: journal tail for {v}: {id:#x}");
        tails.push((v.clone(), id, 0));
    }
    tails
}

fn last_event_id_before_now(volume: &str) -> u64 {
    let c = match CString::new(volume) {
        Ok(c) => c,
        Err(_) => return kFSEventStreamEventIdSinceNow,
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(c.as_ptr(), &mut st) } != 0 {
        return kFSEventStreamEventIdSinceNow;
    }
    let now = cfa_now();
    unsafe { FSEventsGetLastEventIdForDeviceBeforeTime(st.st_dev, now) }
}

/// CFAbsoluteTime = seconds since 2001-01-01 00:00:00 UTC.
fn cfa_now() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() - 978_307_200.0,
        Err(e) => -(e.duration().as_secs_f64()) - 978_307_200.0,
    }
}

/// Watch all volumes for changes via FSEvents, applying events to the index
/// and content store until the process exits. Blocks the calling thread on the
/// current run loop.
#[allow(deprecated)] // FSEventStreamScheduleWithRunLoop is deprecated in favor of dispatch queues.
pub fn watch_all(
    volumes: &[String],
    index: &Arc<FileIndex>,
    content: &Arc<ContentStore>,
    tails: &[(String, u64, i64)],
) -> Result<()> {
    let run_loop = CFRunLoop::current().context("fsevents: no current run loop")?;
    let tail_map: HashMap<&str, u64> = tails.iter().map(|(v, id, _)| (v.as_str(), *id)).collect();

    let state = Box::into_raw(Box::new(WatcherState {
        index: index.clone(),
        content: content.clone(),
        volumes: volumes.to_vec(),
        path_key: CFString::from_str(EXT_DATA_PATH),
        fileid_key: CFString::from_str(EXT_DATA_FILE_ID),
    }));

    let mut streams: Vec<FSEventStreamRef> = Vec::new();
    for v in volumes {
        let since = tail_map
            .get(v.as_str())
            .copied()
            .unwrap_or(kFSEventStreamEventIdSinceNow);
        match start_stream(v, since, state) {
            Ok(stream) => {
                unsafe {
                    FSEventStreamScheduleWithRunLoop(
                        stream,
                        &run_loop,
                        kCFRunLoopDefaultMode.expect("kCFRunLoopDefaultMode"),
                    );
                    if !FSEventStreamStart(stream) {
                        tracing::warn!("fsevents: failed to start stream for {v}");
                        FSEventStreamInvalidate(stream);
                        continue;
                    }
                }
                tracing::info!("fsevents: watching {v} since {since:#x}");
                streams.push(stream);
            }
            Err(e) => tracing::warn!("fsevents: cannot watch {v}: {e:#}"),
        }
    }

    if streams.is_empty() {
        bail!("fsevents: no volume streams could be started");
    }

    // Block forever; FSEvents delivers events to this thread's run loop.
    CFRunLoop::run();
    Ok(())
}

fn start_stream(
    volume: &str,
    since_when: u64,
    state: *mut WatcherState,
) -> Result<FSEventStreamRef> {
    let path = CFString::from_str(volume);
    let paths = CFArray::from_retained_objects(&[path]);
    let mut ctx = FSEventStreamContext {
        version: 0,
        info: state as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let flags = kFSEventStreamCreateFlagFileEvents
        | kFSEventStreamCreateFlagNoDefer
        | kFSEventStreamCreateFlagWatchRoot
        | kFSEventStreamCreateFlagIgnoreSelf
        | kFSEventStreamCreateFlagUseExtendedData;
    let stream = unsafe {
        FSEventStreamCreate(
            None,
            Some(on_fsevent),
            &mut ctx,
            paths.as_opaque(),
            since_when,
            0.5, // latency seconds
            flags,
        )
    };
    if stream.is_null() {
        bail!("fsevents: FSEventStreamCreate failed for {volume}");
    }
    Ok(stream)
}/// FSEvents callback. With `UseExtendedData`, `event_paths` is a CFArray of
/// CFDictionary objects, each carrying `path` (CFString) and `fileID`
/// (CFNumber) keys. `event_flags`/`event_ids` are parallel arrays of length
/// `num_events`.
unsafe extern "C-unwind" fn on_fsevent(
    _stream_ref: ConstFSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: NonNull<c_void>,
    event_flags: NonNull<FSEventStreamEventFlags>,
    _event_ids: NonNull<FSEventStreamEventId>,
) {
    let state = unsafe { &*(info as *const WatcherState) };
    let paths = unsafe { &*(event_paths.as_ptr() as *const CFArray<CFType>) };
    let flags = unsafe { std::slice::from_raw_parts(event_flags.as_ptr(), num_events) };

    for i in 0..num_events {
        let flag = flags[i];
        let Some(elem) = paths.get(i) else { continue };
        let dict = elem.as_ref() as *const CFType as *const CFDictionary<CFString, CFType>;
        let dict = unsafe { &*dict };
        let path: Option<String> = unsafe { dict.get_unchecked(&*state.path_key) }
            .and_then(|v| v.downcast_ref::<CFString>())
            .map(|s| s.to_string());
        let file_id: Option<i64> = unsafe { dict.get_unchecked(&*state.fileid_key) }
            .and_then(|v| v.downcast_ref::<CFNumber>())
            .and_then(|n| n.as_i64());
        let Some(path) = path else { continue };
        apply_event(state, flag, &path, file_id);
    }
}

/// Apply a single FSEvents event to the index and content store, mirroring the
/// fanotify event-application body (DELETE / RENAME+RENAME_NEW / WRITE).
fn apply_event(state: &WatcherState, flag: u32, path: &str, file_id: Option<i64>) {
    let now = now_filetime();
    let is_dir = flag & kFSEventStreamEventFlagItemIsDir != 0;

    // Overflow / resync signals → full re-scan.
    if flag & (kFSEventStreamEventFlagMustScanSubDirs
        | kFSEventStreamEventFlagUserDropped
        | kFSEventStreamEventFlagKernelDropped
        | kFSEventStreamEventFlagEventIdsWrapped
        | kFSEventStreamEventFlagRootChanged) != 0
    {
        tracing::warn!("fsevents: overflow flags {flag:#x} on {path}; full re-scan");
        if let Err(e) = crate::scan::build_index(&state.volumes, &state.index) {
            tracing::warn!("fsevents: overflow re-scan failed: {e:#}");
        }
        return;
    }

    if flag & kFSEventStreamEventFlagItemRemoved != 0 {
        if is_dir {
            state.index.remove_prefix(path);
        } else {
            state.index.remove(path);
        }
        state.index.record_change(now, "DELETE", path, is_dir);
        state.content.remove(path);
        return;
    }

    if flag & kFSEventStreamEventFlagItemRenamed != 0 {
        // FSEvents reports the new path with a fileID; the old path is still
        // in the refs map under that fileID → pair the rename.
        let volume = crate::platform::volume_of(path);
        let old_path = file_id.and_then(|id| state.index.path_by_ref(&volume, id as u64));
        match old_path {
            Some(old) if old != path => {
                if is_dir {
                    state.index.rename_prefix(&old, path);
                    tracing::info!("fsevents: RENAME dir {} -> {}", old, path);
                } else {
                    state.index.remove(&old);
                }
                state.index.record_change(now, "RENAME", &old, is_dir);
                state.index.record_change(now, "RENAME_NEW", path, is_dir);
                state.content.remove(&old);
                state.content.remove(path);
                upsert_or_remove(state, path, is_dir);
            }
            _ => {
                // No old path known (e.g. created during scan window) — treat as WRITE.
                state.index.record_change(now, "WRITE", path, is_dir);
                upsert_or_remove(state, path, is_dir);
            }
        }
        return;
    }

    // Everything else: created / modified / inode-meta / finder-info / owner / xattr / cloned.
    state.index.record_change(now, "WRITE", path, is_dir);
    upsert_or_remove(state, path, is_dir);
}

/// Re-stat a path and upsert it into the index + content store, or remove it if
/// it no longer exists. Mirrors fanotify's `upsert_or_remove`.
fn upsert_or_remove(state: &WatcherState, path: &str, is_dir: bool) {
    if let Some(entry) = crate::walk_macos::stat_path(path, is_dir) {
        if !entry.is_dir && ContentStore::should_index(path, entry.size) {
            if let Ok(data) = std::fs::read(path) {
                let keep = data.len().min(crate::content::MAX_FILE_BYTES as usize);
                state.content.insert(path, &data[..keep]);
            }
        } else {
            state.content.remove(path);
        }
        state.index.upsert(entry);
    } else {
        state.index.remove(path);
        state.content.remove(path);
    }
}

/// Current time as a Windows FILETIME (100ns since 1601-01-01), matching the
/// index's timestamp convention.
fn now_filetime() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64 + FILETIME_EPOCH_OFFSET) * 10_000_000
            + (d.subsec_nanos() as i64 / 100),
        Err(e) => (e.duration().as_secs() as i64 + FILETIME_EPOCH_OFFSET) * 10_000_000,
    }
}