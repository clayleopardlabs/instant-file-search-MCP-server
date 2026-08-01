//! USN Change Journal watcher.
//!
//! After the initial MFT scan, we stay current by replaying the NTFS USN
//! Change Journal (the same mechanism Everything uses). We poll the journal
//! per-volume in a loop and apply create/delete/rename/attribute-change
//! records to the in-memory index.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use windows::Win32::Foundation::HANDLE;
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

const USN_REASON_RENAME: u32 = 0x0000_0001;
const USN_REASON_CLOSE: u32 = 0x8000_0000;
const USN_REASON_DELETE: u32 = 0x0000_0002;
const USN_REASON_CREATE: u32 = 0x0000_0100;
const USN_REASON_HARD_LINK_CHANGE: u32 = 0x0000_0004;

/// Watch all volumes in a loop (one thread per volume is fine; the journal
/// IOCTLs block).
pub fn watch_all(volumes: &[String], index: &Arc<FileIndex>) -> Result<()> {
    let mut handles = Vec::new();
    for v in volumes {
        let idx = index.clone();
        let vol = v.clone();
        handles.push(std::thread::spawn(move || {
            if let Err(e) = watch_one(&vol, &idx) {
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

fn watch_one(volume: &str, index: &Arc<FileIndex>) -> Result<()> {
    let handle = open_volume(volume)?;
    let journal = query_journal(handle)?;
    let journal_id = journal.UsnJournalID;
    let mut next_usn = journal.FirstUsn;
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
        if let Err(e) = ok {
            // Journal gone (deleted by user or fsutil)? Re-query and restart.
            tracing::warn!("FSCTL_READ_USN_JOURNAL on {} failed: {e}", volume);
            std::thread::sleep(Duration::from_secs(10));
            if let Ok(j) = query_journal(handle) {
                next_usn = j.FirstUsn;
            }
            continue;
        }
        if returned == 0 {
            continue;
        }
        // The last 8 bytes are the next USN cursor.
        if returned >= 8 {
            next_usn = i64::from_le_bytes(buf[returned as usize - 8..returned as usize].try_into().unwrap());
        }
        apply_records(volume, index, &buf[..returned as usize - 8]);
    }
}

fn apply_records(volume: &str, index: &Arc<FileIndex>, data: &[u8]) {
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
                    let path = if let Some(p) = resolve_by_ref(index, volume, rec.ParentFileReferenceNumber) {
                        format!("{p}\\{name}")
                    } else {
                        format!("{}\\{}", volume, name)
                    };
                    if reason & USN_REASON_DELETE != 0 && reason & USN_REASON_CREATE == 0 {
                        index.remove(&path);
                    } else if reason & USN_REASON_CLOSE != 0
                        || reason & USN_REASON_CREATE != 0
                        || reason & USN_REASON_RENAME != 0
                        || reason & USN_REASON_HARD_LINK_CHANGE != 0
                    {
                        index.upsert(IndexedFile {
                            path,
                            size: 0,
                            created: rec.TimeStamp as i64,
                            modified: rec.TimeStamp as i64,
                            accessed: rec.TimeStamp as i64,
                            is_dir: rec.FileAttributes & 0x10 != 0,
                            file_ref: rec.FileReferenceNumber,
                        });
                    }
                }
            }
        }
        offset += rec.RecordLength as usize;
    }
}

fn resolve_by_ref(index: &FileIndex, volume: &str, parent_ref: u64) -> Option<String> {
    let _ = volume;
    index.path_by_ref(parent_ref)
}
