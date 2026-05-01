//! Sector-aligned reader over a raw NTFS volume `HANDLE`.
//!
//! Raw volume reads via `ReadFile` must be sector-aligned in both offset
//! and length on Windows. NTFS FILE records are typically 1 KiB while
//! sectors are 512 bytes (or 4 KiB on Advanced Format drives), so we wrap
//! the raw volume handle in a `Read + Seek` adapter that internally
//! buffers a configurable number of sectors. Higher-level code (the MFT
//! iterator) then sees a familiar byte-oriented stream.

use crate::errors::UsnError;
use std::io::{self, Read, Seek, SeekFrom};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx},
};

/// Default buffer size used by `VolumeReader` (256 KiB). Large enough to
/// amortize the cost of `ReadFile`/`SetFilePointerEx` across a few
/// hundred FILE records.
pub const DEFAULT_BUFFER_BYTES: usize = 256 * 1024;

/// Reads from a raw volume `HANDLE` without taking ownership of it.
///
/// The reader maintains its own byte-level cursor; every read is broken
/// into sector-aligned `ReadFile` calls under the hood, with the data
/// copied into the caller's buffer.
pub(crate) struct VolumeReader {
    handle: HANDLE,
    sector_size: u64,
    position: u64,
    buf: Vec<u8>,
    /// Volume offset of the start of `buf`.
    buf_pos: u64,
    /// Number of valid bytes in `buf`.
    buf_len: usize,
}

impl VolumeReader {
    pub fn new(handle: HANDLE, sector_size: u64) -> Result<Self, UsnError> {
        Self::with_buffer_bytes(handle, sector_size, DEFAULT_BUFFER_BYTES)
    }

    pub fn with_buffer_bytes(
        handle: HANDLE,
        sector_size: u64,
        buffer_bytes: usize,
    ) -> Result<Self, UsnError> {
        if !sector_size.is_power_of_two() || sector_size == 0 {
            return Err(UsnError::InvalidBootSector(
                "sector_size must be a non-zero power of two",
            ));
        }
        let sectors = (buffer_bytes as u64 / sector_size).max(1);
        let cap = (sectors * sector_size) as usize;
        Ok(Self {
            handle,
            sector_size,
            position: 0,
            buf: vec![0u8; cap],
            buf_pos: u64::MAX,
            buf_len: 0,
        })
    }

    fn round_down(&self, n: u64) -> u64 {
        n & !(self.sector_size - 1)
    }

    fn raw_seek(&self, offset: u64) -> io::Result<()> {
        let mut new_pos: i64 = 0;
        // SAFETY: `self.handle` is a live volume handle owned by this
        // `VolumeReader`; `&mut new_pos` is a unique stack out-pointer.
        let res =
            unsafe { SetFilePointerEx(self.handle, offset as i64, Some(&mut new_pos), FILE_BEGIN) };
        res.map_err(io::Error::other)?;
        Ok(())
    }

    fn refill(&mut self, sector_pos: u64) -> io::Result<()> {
        self.raw_seek(sector_pos)?;
        let mut bytes_read: u32 = 0;
        // SAFETY: `self.handle` is a live volume handle. The output
        // buffer is `self.buf` of exactly the slice length we pass;
        // `&mut bytes_read` is a unique stack out-pointer. The Win32
        // `ReadFile` requires sector-aligned offsets and lengths for
        // `FILE_FLAG_NO_BUFFERING` opens — the caller (`refill`) is
        // responsible for invoking us with a sector-aligned offset.
        let res = unsafe {
            ReadFile(
                self.handle,
                Some(self.buf.as_mut_slice()),
                Some(&mut bytes_read),
                None,
            )
        };
        res.map_err(io::Error::other)?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "zero-length read from volume",
            ));
        }
        self.buf_pos = sector_pos;
        self.buf_len = bytes_read as usize;
        Ok(())
    }

    fn buf_contains(&self, position: u64, n: usize) -> bool {
        if self.buf_len == 0 {
            return false;
        }
        let buf_end = self.buf_pos + self.buf_len as u64;
        position >= self.buf_pos && position.saturating_add(n as u64) <= buf_end
    }

    /// Borrow a mutable view of `len` bytes at the given volume offset
    /// directly out of the internal buffer, refilling if necessary.
    ///
    /// This skips the per-record memcpy that `Read::read_exact` would
    /// otherwise perform — the caller can fix up and parse the record
    /// in place. Safe because each MFT record's USA fixup only touches
    /// bytes inside that record.
    pub fn borrow_at(&mut self, offset: u64, len: usize) -> io::Result<&mut [u8]> {
        if !self.buf_contains(offset, len) {
            // If the requested range can't fit in the buffer at all,
            // grow the buffer to a sector-aligned size that holds it.
            let needed = (len as u64).div_ceil(self.sector_size) * self.sector_size;
            if (needed as usize) > self.buf.len() {
                self.buf.resize(needed as usize, 0);
            }
            let sector_pos = self.round_down(offset);
            self.refill(sector_pos)?;
            if !self.buf_contains(offset, len) {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short read while borrowing volume buffer",
                ));
            }
        }
        let inner_off = (offset - self.buf_pos) as usize;
        self.position = offset + len as u64;
        Ok(&mut self.buf[inner_off..inner_off + len])
    }
}

impl Read for VolumeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        // Fast path: requested bytes already in the buffered window.
        if !self.buf_contains(self.position, 1) {
            let sector_pos = self.round_down(self.position);
            self.refill(sector_pos)?;
        }
        let inner_off = (self.position - self.buf_pos) as usize;
        let avail = self.buf_len - inner_off;
        let n = out.len().min(avail);
        out[..n].copy_from_slice(&self.buf[inner_off..inner_off + n]);
        self.position += n as u64;
        Ok(n)
    }
}

impl Seek for VolumeReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.position
                        .checked_add(n as u64)
                        .ok_or_else(|| io::Error::other("seek overflow"))?
                } else {
                    self.position
                        .checked_sub((-n) as u64)
                        .ok_or_else(|| io::Error::other("seek underflow"))?
                }
            }
            SeekFrom::End(_) => {
                return Err(io::Error::other("seek from end not supported"));
            }
        };
        self.position = new_pos;
        Ok(new_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_down_aligns_to_sector() {
        let r = VolumeReader {
            handle: HANDLE(std::ptr::null_mut()),
            sector_size: 512,
            position: 0,
            buf: vec![0u8; 4096],
            buf_pos: u64::MAX,
            buf_len: 0,
        };
        assert_eq!(r.round_down(0), 0);
        assert_eq!(r.round_down(511), 0);
        assert_eq!(r.round_down(512), 512);
        assert_eq!(r.round_down(513), 512);
        assert_eq!(r.round_down(1023), 512);
        assert_eq!(r.round_down(4096), 4096);
    }

    #[test]
    fn rejects_non_power_of_two_sector_size() {
        let h = HANDLE(std::ptr::null_mut());
        assert!(VolumeReader::new(h, 0).is_err());
        assert!(VolumeReader::new(h, 3).is_err());
        assert!(VolumeReader::new(h, 6).is_err());
        assert!(VolumeReader::new(h, 512).is_ok());
    }

    #[test]
    fn buf_contains_handles_window() {
        let mut r = VolumeReader {
            handle: HANDLE(std::ptr::null_mut()),
            sector_size: 512,
            position: 0,
            buf: vec![0u8; 4096],
            buf_pos: 1024,
            buf_len: 2048,
        };
        assert!(r.buf_contains(1024, 1));
        assert!(r.buf_contains(1024, 2048));
        assert!(!r.buf_contains(1024, 2049));
        assert!(!r.buf_contains(1023, 1));
        assert!(r.buf_contains(3071, 1));
        assert!(!r.buf_contains(3072, 1));
        r.buf_len = 0;
        assert!(!r.buf_contains(1024, 1));
    }
}
