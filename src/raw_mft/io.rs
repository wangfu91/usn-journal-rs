//! Sector-aligned reader over a raw NTFS volume `HANDLE`.
//!
//! Raw volume reads via `ReadFile` must be sector-aligned in both offset
//! and length on Windows. NTFS FILE records are typically 1 KiB while
//! sectors are 512 bytes (or 4 KiB on Advanced Format drives), so we wrap
//! the raw volume handle in a `Read + Seek` adapter that internally
//! buffers whole sectors. Higher-level code (the MFT iterator) then sees
//! a familiar byte-oriented stream.

use crate::errors::UsnError;
use std::io::{self, Read, Seek, SeekFrom};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{ReadFile, SetFilePointerEx, FILE_BEGIN},
};

/// Reads from a raw volume `HANDLE` without taking ownership of it.
///
/// The reader maintains its own byte-level cursor; every read is broken
/// into sector-aligned `ReadFile` calls under the hood, with the data
/// copied into the caller's buffer.
pub(crate) struct VolumeReader {
    handle: HANDLE,
    sector_size: u64,
    position: u64,
    sector_buf: Vec<u8>,
    sector_buf_pos: u64,
    sector_buf_valid: bool,
}

impl VolumeReader {
    pub fn new(handle: HANDLE, sector_size: u64) -> Result<Self, UsnError> {
        if !sector_size.is_power_of_two() || sector_size == 0 {
            return Err(UsnError::InvalidBootSector(
                "sector_size must be a non-zero power of two",
            ));
        }
        Ok(Self {
            handle,
            sector_size,
            position: 0,
            sector_buf: vec![0u8; sector_size as usize],
            sector_buf_pos: u64::MAX,
            sector_buf_valid: false,
        })
    }

    fn round_down(&self, n: u64) -> u64 {
        n & !(self.sector_size - 1)
    }

    fn raw_seek(&self, offset: u64) -> io::Result<()> {
        let mut new_pos: i64 = 0;
        // SAFETY: passing valid HANDLE; offset fits.
        let res = unsafe {
            SetFilePointerEx(
                self.handle,
                offset as i64,
                Some(&mut new_pos),
                FILE_BEGIN,
            )
        };
        res.map_err(io::Error::other)?;
        Ok(())
    }

    fn raw_read_sector(&mut self, sector_pos: u64) -> io::Result<()> {
        self.raw_seek(sector_pos)?;
        let mut bytes_read: u32 = 0;
        // SAFETY: handle is valid; buffer is owned and large enough.
        let res = unsafe {
            ReadFile(
                self.handle,
                Some(self.sector_buf.as_mut_slice()),
                Some(&mut bytes_read),
                None,
            )
        };
        res.map_err(io::Error::other)?;
        if bytes_read as usize != self.sector_buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short read from volume",
            ));
        }
        self.sector_buf_pos = sector_pos;
        self.sector_buf_valid = true;
        Ok(())
    }
}

impl Read for VolumeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let sector_pos = self.round_down(self.position);
        if !self.sector_buf_valid || sector_pos != self.sector_buf_pos {
            self.raw_read_sector(sector_pos)?;
        }
        let inner_off = (self.position - sector_pos) as usize;
        let avail = self.sector_buf.len() - inner_off;
        let n = buf.len().min(avail);
        buf[..n].copy_from_slice(&self.sector_buf[inner_off..inner_off + n]);
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

    /// In-memory reader satisfying the same contract for testing.
    struct MemReader {
        data: Vec<u8>,
        pos: u64,
    }

    impl Read for MemReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let pos = self.pos as usize;
            if pos >= self.data.len() {
                return Ok(0);
            }
            let n = buf.len().min(self.data.len() - pos);
            buf[..n].copy_from_slice(&self.data[pos..pos + n]);
            self.pos += n as u64;
            Ok(n)
        }
    }

    impl Seek for MemReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.pos = match pos {
                SeekFrom::Start(n) => n,
                SeekFrom::Current(n) => (self.pos as i64 + n) as u64,
                SeekFrom::End(n) => (self.data.len() as i64 + n) as u64,
            };
            Ok(self.pos)
        }
    }

    /// The MemReader stub mirrors the alignment property check in
    /// VolumeReader; we just smoke-test that round_down is a power-of-two
    /// alignment.
    #[test]
    fn round_down_aligns_to_sector() {
        let r = VolumeReader {
            handle: HANDLE(std::ptr::null_mut()),
            sector_size: 512,
            position: 0,
            sector_buf: vec![0u8; 512],
            sector_buf_pos: u64::MAX,
            sector_buf_valid: false,
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
}
