//! Sector-aligned reader for raw volume handles.
//!
//! Reading a raw NTFS volume device (`\\.\C:`) only accepts reads on
//! sector boundaries (typically 4096 bytes). This wrapper ensures every
//! underlying read/seek lands on a sector boundary, so the `ntfs` crate
//! can issue arbitrary-sized reads through it. Pattern taken from the
//! ntfs crate's own `ntfs-shell` example (MIT OR Apache-2.0).

use std::io;
use std::io::{Read, Seek, SeekFrom};

pub struct SectorReader<R>
where
    R: Read + Seek,
{
    inner: R,
    sector_size: usize,
    stream_position: u64,
    temp_buf: Vec<u8>,
}

impl<R> SectorReader<R>
where
    R: Read + Seek,
{
    pub fn new(inner: R, sector_size: usize) -> io::Result<Self> {
        if !sector_size.is_power_of_two() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "sector_size is not a power of two",
            ));
        }
        Ok(Self {
            inner,
            sector_size,
            stream_position: 0,
            temp_buf: Vec::new(),
        })
    }

    fn align_down(&self, n: u64) -> u64 {
        n / self.sector_size as u64 * self.sector_size as u64
    }

    fn align_up(&self, n: u64) -> u64 {
        self.align_down(n) + self.sector_size as u64
    }
}

impl<R> Read for SectorReader<R>
where
    R: Read + Seek,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let buf_start = self.stream_position;
        let buf_end = buf_start + buf.len() as u64;

        let aligned_start = self.align_down(buf_start);
        let aligned_end = self.align_up(buf_end);

        // Read the full aligned range into the temp buffer.
        let needed = (aligned_end - aligned_start) as usize;
        if self.temp_buf.len() < needed {
            self.temp_buf.resize(needed, 0);
        }
        self.inner.seek(SeekFrom::Start(aligned_start))?;
        let mut filled = 0usize;
        while filled < needed {
            let n = self.inner.read(&mut self.temp_buf[filled..needed])?;
            if n == 0 {
                break;
            }
            filled += n;
        }

        // Copy the requested window out.
        let offset = (buf_start - aligned_start) as usize;
        let available = filled.saturating_sub(offset);
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&self.temp_buf[offset..offset + to_copy]);
        self.stream_position += to_copy as u64;
        Ok(to_copy)
    }
}

impl<R> Seek for SectorReader<R>
where
    R: Read + Seek,
{
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let current = self.stream_position;
        let new_pos = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(d) => (current as i64 + d).max(0) as u64,
            SeekFrom::End(d) => {
                let len = self.inner.seek(SeekFrom::End(0))?;
                (len as i64 + d).max(0) as u64
            }
        };
        self.stream_position = new_pos;
        Ok(new_pos)
    }
}
