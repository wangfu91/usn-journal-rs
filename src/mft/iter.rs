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
    volume_handle: HANDLE,
    low_usn: i64,
    high_usn: i64,
    buffer: Vec<u8>,
    bytes_read: u32,
    offset: u32,
    next_start_fid: u64,
}

impl MftIter {
    pub(super) fn new(
        volume_handle: HANDLE,
        low_usn: i64,
        high_usn: i64,
        buffer: Vec<u8>,
        next_start_fid: u64,
    ) -> Self {
        Self {
            volume_handle,
            low_usn,
            high_usn,
            buffer,
            bytes_read: 0,
            offset: 0,
            next_start_fid,
        }
    }

    /// Swap in a caller-provided buffer to avoid allocating during long
    /// iteration loops. The buffer is cleared and resized to the
    /// originally requested capacity. This may be invoked any number of
    /// times before the first `next()` call.
    #[must_use]
    pub fn with_buffer(mut self, buf: Vec<u8>) -> Self {
        let cap = self.buffer.len();
        let mut buf = buf;
        buf.clear();
        buf.resize(cap, 0);
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
            MaxMajorVersion: 3,
        };

        // SAFETY: `self.volume_handle` is a live volume handle. Input
        // points to the stack-local `mft_enum_data` of exactly the size
        // we pass; output points to `self.buffer` of exactly the length
        // we pass; `&mut self.bytes_read` is a unique out-pointer.
        if let Err(err) = unsafe {
            DeviceIoControl(
                self.volume_handle,
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