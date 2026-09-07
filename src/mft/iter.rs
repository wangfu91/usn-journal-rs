//! Iterator over `FSCTL_ENUM_USN_DATA` output buffers.

use std::mem::size_of;

use log::debug;
use windows::Win32::{
    Foundation::{ERROR_HANDLE_EOF, HANDLE},
    System::{
        IO::DeviceIoControl,
        Ioctl::{self, MFT_ENUM_DATA_V1},
    },
};

use crate::{
    UsnResult,
    errors::UsnError,
    usn_record::{self, UsnRecordView},
};

use super::entry::MftEntry;

/// Iterator over MFT entries.
///
/// This iterator yields `Result<MftEntry, UsnError>` items, allowing applications
/// to handle individual entry errors without stopping the entire iteration process.
pub struct MftIter {
    /// Open volume handle used for enumeration.
    volume_handle: std::rc::Rc<windows::core::Owned<HANDLE>>,
    /// Inclusive lower USN bound passed to the kernel.
    low_usn: i64,
    /// Inclusive upper USN bound passed to the kernel.
    high_usn: i64,
    /// Highest `USN_RECORD` major version accepted from the kernel.
    max_usn_record_version: u16,
    /// Scratch buffer reused across `DeviceIoControl` calls.
    buffer: Vec<u8>,
    /// Number of valid bytes currently stored in `buffer`.
    bytes_read: u32,
    /// Current scan offset within `buffer`.
    offset: u32,
    /// File reference number cursor for the next enumeration call.
    next_start_fid: u64,
}

impl MftIter {
    /// Construct an iterator around an open volume handle and enumeration settings.
    pub(super) fn new(
        volume_handle: std::rc::Rc<windows::core::Owned<HANDLE>>,
        low_usn: i64,
        high_usn: i64,
        max_usn_record_version: u16,
        buffer: Vec<u8>,
    ) -> Self {
        Self {
            volume_handle,
            low_usn,
            high_usn,
            max_usn_record_version,
            buffer,
            bytes_read: 0,
            offset: 0,
            // Enumeration always starts at file reference number 0; the kernel
            // returns the cursor for the next call as the first 8 bytes of each
            // output buffer. The USN range is filtered via `low_usn`/`high_usn`.
            next_start_fid: 0,
        }
    }

    /// Replace the scratch buffer, preserving unread records and the cursor.
    /// The supplied buffer is resized to the configured length. This may be
    /// called before or during iteration.
    #[must_use]
    pub fn with_buffer(mut self, buf: Vec<u8>) -> Self {
        let cap = self.buffer.len();
        let mut buf = buf;
        buf.clear();
        buf.resize(cap, 0);
        if self.offset < self.bytes_read {
            buf[self.offset as usize..self.bytes_read as usize]
                .copy_from_slice(&self.buffer[self.offset as usize..self.bytes_read as usize]);
        }
        self.buffer = buf;
        self
    }

    /// Reads the next chunk of MFT data into the buffer.
    ///
    /// Returns `Ok(true)` if data was read, `Ok(false)` if EOF, or an error.
    fn get_data(&mut self) -> Result<bool, UsnError> {
        // To enumerate files on a volume, use the FSCTL_ENUM_USN_DATA operation one or more times.
        // On the first call, set the starting point, the StartFileReferenceNumber member of the MFT_ENUM_DATA structure, to (DWORDLONG)0.
        let mft_enum_data = MFT_ENUM_DATA_V1 {
            StartFileReferenceNumber: self.next_start_fid,
            LowUsn: self.low_usn,
            HighUsn: self.high_usn,
            MinMajorVersion: 2,
            MaxMajorVersion: self.max_usn_record_version,
        };

        // SAFETY: `self.volume_handle` is a live volume handle. Input
        // points to the stack-local `mft_enum_data` of exactly the size
        // we pass; output points to `self.buffer` of exactly the length
        // we pass; `&mut self.bytes_read` is a unique out-pointer.
        if let Err(err) = unsafe {
            DeviceIoControl(
                **self.volume_handle,
                Ioctl::FSCTL_ENUM_USN_DATA,
                Some(&mft_enum_data as *const _ as _),
                size_of::<MFT_ENUM_DATA_V1>() as u32,
                Some(self.buffer.as_mut_ptr() as _),
                self.buffer.len() as u32,
                Some(&mut self.bytes_read),
                None,
            )
        } {
            if err.code() == ERROR_HANDLE_EOF.into() {
                return Ok(false);
            }
            return Err(UsnError::WinApi(err));
        }
        Ok(true)
    }

    /// Finds the next USN record in the buffer, reading more data if needed.
    ///
    /// Returns `Ok(Some(record))` if a record is found, `Ok(None)` if EOF, or an error.
    fn find_next_entry(&mut self) -> Result<Option<UsnRecordView<'_>>, UsnError> {
        if self.offset < self.bytes_read {
            return usn_record::find_next_record(&self.buffer, self.bytes_read, &mut self.offset);
        }

        // We need to read more data
        if self.get_data()? {
            // Each call to FSCTL_ENUM_USN_DATA retrieves the starting point for the subsequent call as the first entry in the output buffer.
            self.next_start_fid = usn_record::read_next_start_fid(&self.buffer, self.bytes_read)?;
            self.offset = size_of::<u64>() as u32;

            return usn_record::find_next_record(&self.buffer, self.bytes_read, &mut self.offset);
        }

        // EOF, no more data to read
        Ok(None)
    }
}

impl Iterator for MftIter {
    type Item = UsnResult<MftEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.find_next_entry() {
            Ok(Some(record)) => Some(Ok(MftEntry::new(record))),
            Ok(None) => None,
            Err(err) => {
                debug!("Error finding next MFT entry: {err}");
                Some(Err(err))
            }
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use crate::test_support::mock_volume;
    #[test]
    fn iterator_keeps_handle_after_volume_and_clone_drop() {
        let volume = mock_volume();
        let cloned = volume.clone();
        let weak = std::rc::Rc::downgrade(&volume.handle);
        let iter = volume.mft().into_iter();
        drop(volume);
        drop(cloned);
        assert!(weak.upgrade().is_some());
        drop(iter);
        assert!(weak.upgrade().is_none());
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::*;
    #[test]
    fn replacement_preserves_unread_records() {
        let volume = crate::test_support::mock_volume();
        let mut buffer = vec![0u8; 136];
        for (offset, name) in [(8usize, 'a'), (72, 'b')] {
            buffer[offset..offset + 4].copy_from_slice(&64u32.to_le_bytes());
            buffer[offset + 4..offset + 6].copy_from_slice(&2u16.to_le_bytes());
            buffer[offset + 56..offset + 58].copy_from_slice(&2u16.to_le_bytes());
            buffer[offset + 58..offset + 60].copy_from_slice(&60u16.to_le_bytes());
            buffer[offset + 60..offset + 62].copy_from_slice(&(name as u16).to_le_bytes());
        }
        let mut iter = MftIter::new(volume.shared_handle(), 0, i64::MAX, 3, buffer);
        iter.offset = 8;
        iter.bytes_read = 136;
        assert_eq!(iter.next().unwrap().unwrap().file_name, "a");
        let mut iter = iter.with_buffer(Vec::new());
        assert_eq!(iter.next().unwrap().unwrap().file_name, "b");
        assert_eq!(iter.offset, 136);
        // An exhausted or empty kernel response has no pending bytes to copy.
        iter.bytes_read = 0;
        let _ = iter.with_buffer(Vec::new());
    }
}
