//! MFT scanning via raw sequential reads of the $MFT data stream.
//!
//! The generic `ntfs.file()` API re-parses the MFT header + attribute list on
//! every record, which makes full-volume enumeration far too slow. Everything
//! instead reads the raw MFT data stream in large blocks and parses FILE
//! records manually. This module does the same:
//!
//! 1. Open the volume with a `SectorReader` (raw volume handles reject
//!    unaligned reads with ERROR_INVALID_PARAMETER).
//! 2. Read the `$MFT` file record, find its `$DATA` attribute, and get its
//!    value as a `Read + Seek` stream over the data runs.
//! 3. Read the stream in 1 MiB blocks and parse each FILE record by hand:
//!    signature `FILE`, update-sequence-array fixup, attribute walk.
//! 4. Collect `FILE_NAME` attributes (parent ref, name, times, size, flags).

use std::collections::HashMap;
use std::io::{BufReader, SeekFrom};

use anyhow::{Context, Result};
use ntfs::{KnownNtfsFileRecordNumber, Ntfs, NtfsReadSeek};

use crate::sector_reader::SectorReader;

/// A single indexed file with full path.
#[derive(Debug, Clone)]
pub struct IndexedFile {
    /// Absolute path like `C:\Windows\System32\notepad.exe`.
    pub path: String,
    /// File size in bytes (0 for directories).
    pub size: u64,
    /// Creation time, 100 ns since 1601 (FILETIME).
    pub created: i64,
    /// Last modification time, 100 ns since 1601 (FILETIME).
    pub modified: i64,
    /// Last access time, 100 ns since 1601 (FILETIME).
    pub accessed: i64,
    /// `true` for directories.
    pub is_dir: bool,
    /// NTFS file record number (used as the USN file reference).
    pub file_ref: u64,
    /// Precomputed lowercase name (query hot path).
    pub name: String,
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
            file_ref,
            name: String::new(),
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
            .rsplit('\\')
            .next()
            .unwrap_or_default()
            .to_string();
        self.lower_path = self.path.to_ascii_lowercase();
        self.extension = if self.is_dir {
            None
        } else {
            self.path.rsplit_once('.').and_then(|(head, ext)| {
                if head.is_empty() || ext.is_empty() || ext.contains('\\') {
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

const ATTR_STD_INFO: u32 = 0x10;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

use crate::query::DEFAULT_EXCLUDES;

fn is_default_excluded(lower_path: &str) -> bool {
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

/// Discover NTFS volumes (drive letters) on this machine.
pub fn discover_ntfs_volumes() -> Vec<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};

    let mut volumes = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    eprintln!("discover: drive mask = 0x{mask:X}");
    for i in 0..26u8 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i) as char;
        let root = format!("{letter}:\\");
        let mut root_wide: Vec<u16> = root.encode_utf16().collect();
        root_wide.push(0);
        let drive_type = unsafe { GetDriveTypeW(PCWSTR(root_wide.as_mut_ptr())) };
        eprintln!("discover: {root} drive_type={drive_type}");
        if drive_type != 3 {
            continue;
        }
        let mut fs_name = [0u16; 64];
        let ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(root_wide.as_mut_ptr()),
                None,
                None,
                None,
                None,
                Some(&mut fs_name),
            )
        };
        if ok.is_err() {
            eprintln!("discover: {root} GetVolumeInformationW error: {:?}", ok.err());
            continue;
        }
        let fs =
            String::from_utf16_lossy(&fs_name[..fs_name.iter().position(|&c| c == 0).unwrap_or(0)])
                .trim()
                .to_string();
        eprintln!("discover: {root} fs={fs}");
        if fs.eq_ignore_ascii_case("NTFS") {
            volumes.push(root);
        }
    }
    volumes
}

/// Scan one volume, returning all indexed files.
///
/// Fast path: the whole $MFT is read sequentially in blocks and FILE records
/// are parsed in memory — no per-record API calls.
pub fn scan_volume(volume: &str) -> Result<Vec<IndexedFile>> {
    let device = format!(r"\\?\GLOBALROOT\Device\HarddiskVolume",);
    let _ = device;

    // The volume root `C:\` maps to a device; use the plain volume path.
    let f = std::fs::File::open(&format!(r"\\?\{}", volume)).or_else(|_| {
        std::fs::File::open(&format!(r"\\.\{}", volume.trim_end_matches('\\')))
    })
    .context("open volume device (requires admin/SYSTEM)")?;
    let sector = SectorReader::new(f, 4096)?;
    let mut buffered = BufReader::with_capacity(1 << 20, sector);

    let ntfs = Ntfs::new(&mut buffered).context("parse boot sector")?;
    let record_size = ntfs.file_record_size() as usize;

    let mft = ntfs
        .file(&mut buffered, KnownNtfsFileRecordNumber::MFT as u64)
        .context("read $MFT record")?;
    let item = mft
        .data(&mut buffered, "")
        .context("find $MFT $DATA")?
        .context("$MFT $DATA attribute error")?;
    let data = item.to_attribute().context("read $MFT $DATA attribute")?;
    let mut stream = data.value(&mut buffered).context("get $MFT $DATA value")?;

    let total = stream
        .seek(&mut buffered, SeekFrom::End(0))
        .context("get $MFT size")?;
    let record_count = (total / record_size as u64) as usize;
    eprintln!("scan_volume: mft_size={total} record_size={record_size} record_count={record_count}");
    stream.seek(&mut buffered, SeekFrom::Start(0)).ok();

    let mut entries: Vec<IndexedFile> = Vec::with_capacity(record_count);
    let mut names: HashMap<u64, (u64, String)> = HashMap::with_capacity(record_count);

    let mut record_number: u64 = 0;
    let mut carry: Vec<u8> = Vec::with_capacity((1 << 20) + record_size);
    let mut block = vec![0u8; 1 << 20];

    let mut read_remaining = total as usize;
    while read_remaining > 0 {
        let want = block.len().min(read_remaining);
        let mut filled = 0usize;
        while filled < want {
            let n = stream
                .read(&mut buffered, &mut block[filled..want])
                .context("read $MFT block")?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        read_remaining -= filled;

        carry.extend_from_slice(&block[..filled]);
        let mut consumed = 0usize;
        while carry.len() - consumed >= record_size {
            let rec = &carry[consumed..consumed + record_size];
            if let Some((entry, pname)) = parse_file_record(rec, record_number) {
                if let Some((parent, n)) = pname {
                    names.insert(record_number, (parent, n));
                }
                entries.push(entry);
            }
            consumed += record_size;
            record_number += 1;
        }
        carry.drain(..consumed);

        if record_number % 250_000 == 0 {
            eprintln!("scan_volume: parsed {record_number}/{record_count} records");
        }
    }

    eprintln!(
        "scan_volume: {} records parsed, {} files, {} names",
        record_number,
        entries.len(),
        names.len()
    );

    resolve_paths(&mut entries, &names, volume);
    Ok(entries)
}

/// Parse one FILE record; returns the entry and its (parent_ref, name).
fn parse_file_record(buf: &[u8], record_number: u64) -> Option<(IndexedFile, Option<(u64, String)>)> {
    if buf.len() < 56 || &buf[0..4] != b"FILE" {
        return None;
    }
    let flags = u16::from_le_bytes([buf[0x16], buf[0x17]]);
    if flags & 0x01 == 0 {
        return None; // not in use
    }
    let is_dir = flags & 0x02 != 0;

    // Update Sequence Array fixup: the last 2 bytes of every 512-byte sector
    // hold the original bytes; the array at usa_offset contains them. Without
    // restoration, a FILE_NAME spanning a sector boundary reads garbage.
    let usa_offset = u16::from_le_bytes([buf[4], buf[5]]) as usize;
    let usa_count = u16::from_le_bytes([buf[6], buf[7]]) as usize;
    if usa_offset > 0 && usa_count >= 2 {
        let sector_size = 512usize;
        let first_seq = u16::from_le_bytes([buf[usa_offset], buf[usa_offset + 1]]);
        let sectors_ok = buf.len() >= usa_offset + usa_count * 2 && buf.len() % sector_size == 0;
        if sectors_ok {
            let mut ok = true;
            for i in 1..usa_count {
                let sector_end = i * sector_size;
                if sector_end > buf.len() {
                    ok = false;
                    break;
                }
                if u16::from_le_bytes([buf[sector_end - 2], buf[sector_end - 1]]) != first_seq {
                    ok = false;
                    break;
                }
            }
            if ok {
                let mut fixed = buf.to_vec();
                for i in 1..usa_count {
                    let sector_end = i * sector_size;
                    let src = usa_offset + i * 2;
                    let orig = u16::from_le_bytes([fixed[src], fixed[src + 1]]);
                    fixed[sector_end - 2] = (orig & 0xFF) as u8;
                    fixed[sector_end - 1] = (orig >> 8) as u8;
                }
                return parse_file_record_inner(&fixed, record_number, is_dir);
            }
        }
    }
    parse_file_record_inner(buf, record_number, is_dir)
}

fn parse_file_record_inner(
    buf: &[u8],
    record_number: u64,
    is_dir: bool,
) -> Option<(IndexedFile, Option<(u64, String)>)> {
    let first_attr = u16::from_le_bytes([buf[0x14], buf[0x15]]) as usize;
    let mut off = first_attr;

    let mut parent_ref: Option<u64> = None;
    let mut name: Option<String> = None;
    let mut created = 0i64;
    let mut modified = 0i64;
    let mut accessed = 0i64;
    let mut size = 0u64;
    let mut data_seen = false;
    let mut name_ns = i8::MAX;

    while off + 8 <= buf.len() {
        let atype =
            u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        if atype == ATTR_END {
            break;
        }
        let alen = u32::from_le_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]])
            as usize;
        if alen < 24 || off + alen > buf.len() {
            break;
        }
        let non_resident = buf[off + 8] != 0;
        if atype == ATTR_STD_INFO && !non_resident {
            let value_len = u32::from_le_bytes([
                buf[off + 16],
                buf[off + 17],
                buf[off + 18],
                buf[off + 19],
            ]) as usize;
            let value_off =
                u16::from_le_bytes([buf[off + 20], buf[off + 21]]) as usize;
            let v = &buf[off + value_off..(off + value_off + value_len).min(buf.len())];
            if v.len() >= 32 {
                created = i64::from_le_bytes(v[0..8].try_into().ok()?);
                modified = i64::from_le_bytes(v[8..16].try_into().ok()?);
                accessed = i64::from_le_bytes(v[24..32].try_into().ok()?);
            }
        } else if atype == ATTR_FILE_NAME && !non_resident {
            let value_len = u32::from_le_bytes([
                buf[off + 16],
                buf[off + 17],
                buf[off + 18],
                buf[off + 19],
            ]) as usize;
            let value_off =
                u16::from_le_bytes([buf[off + 20], buf[off + 21]]) as usize;
            let v = &buf[off + value_off..(off + value_off + value_len).min(buf.len())];
            if v.len() >= 66 {
                let p = u64::from_le_bytes(v[0..8].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;
                let name_len = v[64] as usize;
                let ns = v[65] as i8;
                if name_len > 0 && v.len() >= 66 + name_len * 2 {
                    let n = String::from_utf16_lossy(
                        &v[66..66 + name_len * 2]
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect::<Vec<_>>(),
                    );
                    if ns <= name_ns {
                        name_ns = ns;
                        parent_ref = Some(p);
                        name = Some(n);
                    }
                }
            }
        } else if atype == ATTR_DATA {
            if non_resident {
                if !data_seen && off + 56 <= buf.len() {
                    size = u64::from_le_bytes(buf[off + 48..off + 56].try_into().ok()?);
                    data_seen = true;
                }
            } else if !data_seen {
                let value_len = u32::from_le_bytes([
                    buf[off + 16],
                    buf[off + 17],
                    buf[off + 18],
                    buf[off + 19],
                ]) as usize;
                size = value_len as u64;
                data_seen = true;
            }
        }
        off += alen;
    }

    let parent = parent_ref?;
    let name = name?;
    Some((
        IndexedFile::new(
            String::new(),
            size,
            created,
            modified,
            accessed,
            is_dir,
            record_number,
        ),
        Some((parent, name)),
    ))
}

/// Build the full path for every entry by walking parent references.
///
/// `names` maps file_ref -> (parent_ref, name). The volume root (record 5)
/// resolves to `C:\`.
pub fn resolve_paths(
    entries: &mut [IndexedFile],
    names: &HashMap<u64, (u64, String)>,
    volume_root: &str,
) {
    let mut cache: HashMap<u64, String> = HashMap::new();
    let root = volume_root.trim_end_matches('\\');
    let root_ref = KnownNtfsFileRecordNumber::RootDirectory as u64;

    for e in entries.iter_mut() {
        let mut refs: Vec<u64> = Vec::new();
        let mut cur = e.file_ref;
        loop {
            if let Some(cached) = cache.get(&cur) {
                let mut path = cached.clone();
                for r in refs.iter().rev() {
                    if let Some((_, n)) = names.get(r) {
                        path.push('\\');
                        path.push_str(n);
                    }
                }
                cache.insert(e.file_ref, path.clone());
                e.set_path(path);
                break;
            }
            if cur == root_ref {
                let mut path = String::from(root);
                for r in refs.iter().rev() {
                    if let Some((_, n)) = names.get(r) {
                        path.push('\\');
                        path.push_str(n);
                    }
                }
                cache.insert(e.file_ref, path.clone());
                e.set_path(path);
                break;
            }
            match names.get(&cur) {
                Some((parent, n)) => {
                    refs.push(cur);
                    let _ = n;
                    cur = *parent;
                }
                None => {
                    e.set_path(String::new());
                    break;
                }
            }
        }
    }
}
