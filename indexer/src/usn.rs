//! USN Change Journal watcher.
//!
//! After the initial MFT scan, we stay current by replaying the NTFS USN
//! Change Journal (the same mechanism Everything uses). We poll the journal
//! per-volume in a loop and apply create/delete/rename/attribute-change
//! records to the in-memory index.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
    USN_RECORD_V2,
};
use windows::core::PCWSTR;

use crate::index::FileIndex;
use crate::mft::IndexedFile;

const USN_REASON_RENAME_OLD: u32 = 0x0000_1000;
const USN_REASON_RENAME_NEW: u32 = 0x0000_2000;
const USN_REASON_CLOSE: u32 = 0x8000_0000;
const USN_REASON_DELETE: u32 = 0x0000_0200;
const USN_REASON_CREATE: u32 = 0x0000_0100;
const USN_REASON_HARD_LINK_CHANGE: u32 = 0x0001_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

/// Watch all volumes in a loop (one thread per volume is fine; the journal
/// IOCTLs block).
pub fn journal_tails(volumes: &[String]) -> Vec<(String, u64, i64)> {
    let mut tails = Vec::new();
    for v in volumes {
        if let Ok(h) = open_volume(v) {
            if let Ok(j) = query_journal(h) {
                tails.push((v.clone(), j.UsnJournalID, j.NextUsn));
            }
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
    tails
}

pub fn watch_all(volumes: &[String], index: &Arc<FileIndex>, content: &Arc<crate::content::ContentStore>, tails: &[(String, u64, i64)]) -> Result<()> {
    let mut handles = Vec::new();
    for v in volumes {
        let idx = index.clone();
        let cts = content.clone();
        let vol = v.clone();
        let start = tails
            .iter()
            .find(|(t, _, _)| *t == *v)
            .map(|(_, id, usn)| (*id, *usn))
            .unwrap_or((0, 0));
        handles.push(std::thread::spawn(move || {
            if let Err(e) = watch_one(&vol, &idx, &cts, start) {
                tracing::warn!("USN watcher for {} exited: {e:#}", vol);
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn open_volume(volume: &str) -> Result<HANDLE> {
    let device = format!("\\\\.\\{}", volume.trim_end_matches('\\'));
    let mut wide: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_mut_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
    }?;
    Ok(handle)
}

fn query_journal(handle: HANDLE) -> Result<USN_JOURNAL_DATA_V0> {
    let mut out = USN_JOURNAL_DATA_V0::default();
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some((&mut out as *mut USN_JOURNAL_DATA_V0).cast()),
            std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .context("FSCTL_QUERY_USN_JOURNAL")?;
    Ok(out)
}

fn watch_one(volume: &str, index: &Arc<FileIndex>, content: &Arc<crate::content::ContentStore>, start: (u64, i64)) -> Result<()> {
    let handle = open_volume(volume)?;
    let journal = query_journal(handle)?;
    let mut journal_id = start.0;
    let mut next_usn = start.1;
    // RENAME_OLD_NAME records carry the old path; the matching NEW_NAME
    // record arrives next. For directories we re-prefix the whole subtree
    // (NTFS emits no per-child rename records).
    let mut pending_renames: HashMap<u64, String> = HashMap::new();
    tracing::info!("USN journal on {}: id={journal_id} first={}", volume, journal.FirstUsn);

    let mut buf = vec![0u8; 1 << 20];
    loop {
        std::thread::sleep(Duration::from_millis(2000));
        let mut read = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: next_usn,
            ReasonMask: u32::MAX,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: journal_id,
        };
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_READ_USN_JOURNAL,
                Some((&mut read as *mut READ_USN_JOURNAL_DATA_V0).cast()),
                std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                Some(buf.as_mut_ptr().cast()),
                buf.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        tracing::debug!("USN {}: read rc={}", volume, returned);
        if let Err(e) = ok {
            // Journal rolled over (0x8007049D) or was deleted/recreated: replaying
            // from FirstUsn races the truncation loop (a 32MB journal can wrap
            // again mid-replay and swallow fresh records). A full volume re-scan
            // is authoritative and fast (~14s for 2.4M files).
            tracing::warn!("FSCTL_READ_USN_JOURNAL on {} failed: {e}; rescanning", volume);
            std::thread::sleep(Duration::from_secs(1));
            if let Ok(j) = query_journal(handle) {
                journal_id = j.UsnJournalID;
                match crate::mft::scan_volume(volume) {
                    Ok(entries) => {
                        let n = index.replace_volume(&format!("{volume}\\"), entries);
                        tracing::info!("rescan {}: {n} entries; journal id={journal_id}", volume);
                        next_usn = j.NextUsn;
                    }
                    Err(re) => {
                        tracing::warn!("rescan {} failed: {re:#}", volume);
                        next_usn = j.FirstUsn;
                    }
                }
            }
            continue;
        }
        if returned == 0 {
            tracing::debug!("USN {}: returned=0 at next={}", volume, next_usn);
            continue;
        }
        // The output buffer is: [8-byte next-USN cursor][USN_RECORD_V2 records...]
        // (per MSDN "Walking a Buffer of Change Journal Records")
        if returned > 8 {
            next_usn = i64::from_le_bytes(buf[0..8].try_into().unwrap());
            apply_records(volume, index, content, &buf[8..returned as usize], &mut pending_renames);
        }
        tracing::info!("USN {}: returned={} next={}", volume, returned, next_usn);
    }
}

fn apply_records(
    volume: &str,
    index: &Arc<FileIndex>,
    content: &Arc<crate::content::ContentStore>,
    data: &[u8],
    pending_renames: &mut HashMap<u64, String>,
) {
    let mut offset = 0usize;
    while offset + std::mem::size_of::<USN_RECORD_V2>() <= data.len() {
        let rec = unsafe { &*(data.as_ptr().add(offset) as *const USN_RECORD_V2) };
        if rec.RecordLength == 0 {
            break;
        }
        if rec.RecordLength >= std::mem::size_of::<USN_RECORD_V2>() as u32
            && rec.RecordLength as usize <= data.len() - offset
        {
            let name_len = rec.FileNameLength as usize;
            let name_start = offset + rec.FileNameOffset as usize;
            if name_start + name_len <= data.len() {
                let name = String::from_utf16_lossy(
                    &data[name_start..name_start + name_len]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect::<Vec<_>>(),
                );
                if !name.is_empty() {
                    let reason = rec.Reason;
                    let file_ref = rec.FileReferenceNumber & 0x0000_FFFF_FFFF_FFFF;
                    let is_dir = rec.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                    let path = if let Some(p) = resolve_by_ref(index, volume, rec.ParentFileReferenceNumber) {
                        format!("{p}\\{name}")
                    } else {
                        format!("{}\\{name}", volume.trim_end_matches('\\'))
                    };
                    tracing::debug!("USN {}: name={} reason=0x{:X} path={}", volume, name, reason, path);
                    if reason & USN_REASON_DELETE != 0 {
                        // DELETE takes precedence: a trailing CLOSE record in
                        // the same delete sequence must not re-add the file.
                        if is_dir {
                            // Deleting a directory: NTFS emits no per-child
                            // DELETE records, so drop the whole subtree.
                            index.remove_prefix(&path);
                        } else {
                            index.remove(&path);
                        }
                        pending_renames.remove(&file_ref);
                        index.record_change(rec.TimeStamp, "DELETE", &path, is_dir);
                        content.remove(&path);
                        tracing::debug!("USN {}: DELETE {}", volume, path);
                    } else if reason & USN_REASON_RENAME_OLD != 0 {
                        // Remember the old path so the matching NEW_NAME
                        // record can re-prefix the subtree. The old path no
                        // longer exists on disk; drop it from the index.
                        // Directories keep their entry (with its recursive
                        // size) so rename_prefix can move the whole subtree
                        // including the dir entry itself.
                        pending_renames.insert(file_ref, path.clone());
                        if !is_dir {
                            index.remove(&path);
                        }
                        index.record_change(rec.TimeStamp, "RENAME", &path, is_dir);
                        content.remove(&path);
                        tracing::debug!("USN {}: RENAME_OLD {} (ref {file_ref})", volume, path);
                    } else if reason & USN_REASON_RENAME_NEW != 0 {
                        if let Some(old_path) = pending_renames.remove(&file_ref) {
                            if is_dir && old_path != path {
                                // Renaming a directory: re-prefix every entry
                                // under the old path (no per-child records).
                                index.rename_prefix(&old_path, &path);
                                tracing::info!(
                                    "USN {}: RENAME dir {} -> {} (ref {file_ref})",
                                    volume, old_path, path
                                );
                            }
                        }
                        // The new path exists on disk; (re)index it. A trailing
                        // CLOSE record would do this anyway, but the rename
                        // record may be the last one we see for this file.
                        index.record_change(rec.TimeStamp, "RENAME_NEW", &path, is_dir);
                        content.remove(&path);
                        upsert_or_remove(index, content, &path, rec);
                    } else if reason & USN_REASON_CLOSE != 0
                        || reason & USN_REASON_CREATE != 0
                        || reason & USN_REASON_HARD_LINK_CHANGE != 0
                    {
                        index.record_change(rec.TimeStamp, "WRITE", &path, is_dir);
                        upsert_or_remove(index, content, &path, rec);
                    }
                }
            }
        }
        offset += rec.RecordLength as usize;
    }
}

/// USN records carry no file size; stat on CLOSE/CREATE so size filters stay
/// correct for changed files. If the file is gone (deleted before its CLOSE
/// record, renamed away, or a delete we missed), don't re-add it.
fn upsert_or_remove(index: &Arc<FileIndex>, content: &Arc<crate::content::ContentStore>, path: &str, rec: &USN_RECORD_V2) {
    let mut entry = IndexedFile::new(
        path.to_string(),
        0,
        rec.TimeStamp as i64,
        rec.TimeStamp as i64,
        rec.TimeStamp as i64,
        rec.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        rec.FileReferenceNumber & 0x0000_FFFF_FFFF_FFFF,
    );
    entry.attributes = rec.FileAttributes;
    if let Ok(md) = std::fs::metadata(path) {
        if md.is_dir() != entry.is_dir {
            entry.is_dir = md.is_dir();
        }
        // Index stores Windows FILETIME (100ns since 1601),
        // same convention as the MFT scan.
        const FILETIME_EPOCH: i64 = 116_444_736_000_000_000;
        let to_filetime = |t: std::time::SystemTime| -> i64 {
            match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => FILETIME_EPOCH + (d.as_secs() as i64) * 10_000_000
                    + (d.subsec_nanos() as i64 / 100),
                Err(e) => FILETIME_EPOCH - (e.duration().as_secs() as i64) * 10_000_000,
            }
        };
        entry.size = md.len();
        entry.created = to_filetime(md.created().unwrap_or(std::time::UNIX_EPOCH));
        entry.modified = to_filetime(md.modified().unwrap_or(std::time::UNIX_EPOCH));
        entry.accessed = to_filetime(md.accessed().unwrap_or(std::time::UNIX_EPOCH));
        if !entry.is_dir
            && crate::content::ContentStore::should_index(path, entry.size)
        {
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

fn resolve_by_ref(index: &FileIndex, volume: &str, parent_ref: u64) -> Option<String> {
    // USN refs are full 64-bit file references; the index is keyed by
    // the 48-bit record number (same masking as the MFT scan). The volume
    // is required because record numbers are only unique within a volume.
    index.path_by_ref(volume, parent_ref & 0x0000_FFFF_FFFF_FFFF)
}
