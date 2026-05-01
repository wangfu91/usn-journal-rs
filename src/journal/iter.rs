//! Iterator over USN journal records.

use std::{ffi::c_void, mem::size_of};

use log::{debug, warn};
use windows::Win32::Foundation::{ERROR_HANDLE_EOF, HANDLE};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V1,
};

use crate::{
    UsnResult,
    usn_record::{self, UsnRecordRef},
};

use super::entry::UsnEntry;

/// Iterate over USN journal entries.
///
/// This iterator yields `Result<UsnEntry, UsnError>` items.
pub struct UsnJournalIter {
    pub(super) volume_handle: HANDLE,
    pub(super) journal_id: u64,
    pub(super) buffer: Vec<u8>,
    pub(super) bytes_read: u32,
    pub(super) offset: u32,
    pub(super) next_start_usn: i64,
    pub(super) reason_mask: u32,
    pub(super) return_only_on_close: u32,
    pub(super) timeout: u64,
    pub(super) bytes_to_wait_for: u64,
}

impl UsnJournalIter {
    /// Swap in a caller-provided buffer to avoid allocating during long
    /// iteration loops. The buffer is cleared and resized to the
    /// originally requested capacity. Purely additive; may be called
    /// before the first `next()`.
    #[must_use]
    pub fn with_buffer(mut self, buf: Vec<u8>) -> Self {
        let cap = self.buffer.len();
        let mut buf = buf;
        buf.clear();
        buf.resize(cap, 0);
        self.buffer = buf;
        self
    }

    /// Read the next chunk of USN journal data into the buffer.
    ///
    /// Returns `Ok(true)` if data was read, `Ok(false)` if EOF, or an error.
    fn get_data(&mut self) -> windows::core::Result<bool> {
        let read_data = READ_USN_JOURNAL_DATA_V1 {
            StartUsn: self.next_start_usn,
            ReasonMask: self.reason_mask,
            ReturnOnlyOnClose: self.return_only_on_close,
            Timeout: self.timeout,
            BytesToWaitFor: self.bytes_to_wait_for,
            UsnJournalID: self.journal_id,
            MinMajorVersion: 2,
            MaxMajorVersion: 3,
        };

        // SAFETY: `self.volume_handle` is a live volume handle owned by
        // the journal (validated when the iterator was constructed).
        // `&read_data` and `self.buffer` are valid for the durations of
        // their respective in/out parameters; the input/output sizes
        // exactly match the buffers. `&mut self.bytes_read` is a unique
        // out-pointer. `DeviceIoControl` reports failure via `Result`,
        // which we propagate.
        if let Err(err) = unsafe {
            DeviceIoControl(
                self.volume_handle,
                FSCTL_READ_USN_JOURNAL,
                Some(&read_data as *const _ as *mut _),
                size_of::<READ_USN_JOURNAL_DATA_V1>() as u32,
                Some(self.buffer.as_mut_ptr() as *mut c_void),
                self.buffer.len() as u32,
                Some(&mut self.bytes_read),
                None,
            )
        } {
            if err.code() == ERROR_HANDLE_EOF.into() {
                return Ok(false);
            }

            warn!("Error reading USN data: {err}");
            return Err(err);
        }

        Ok(true)
    }

    /// Find the next USN record in the buffer, reading more data if needed.
    ///
    /// Returns `Ok(Some(record))` if a record is found, `Ok(None)` if EOF, or an error.
    fn find_next_entry(&mut self) -> UsnResult<Option<UsnRecordRef<'_>>> {
        if self.offset < self.bytes_read {
            return usn_record::find_next_record(&self.buffer, self.bytes_read, &mut self.offset);
        }

        // We need to read more data
        if self.get_data()? {
            // https://learn.microsoft.com/en-us/windows/win32/fileio/walking-a-buffer-of-change-journal-records
            // The USN returned as the first item in the output buffer is the USN of the next record number to be retrieved.
            // Use this value to continue reading records from the end boundary forward.
            self.next_start_usn =
                usn_record::read_next_start_usn(&self.buffer, self.bytes_read)?.get();
            self.offset = std::mem::size_of::<i64>() as u32;

            return usn_record::find_next_record(&self.buffer, self.bytes_read, &mut self.offset);
        }

        // EOF, no more data to read
        Ok(None)
    }
}

impl Iterator for UsnJournalIter {
    type Item = UsnResult<UsnEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.find_next_entry() {
            Ok(Some(record)) => Some(Ok(UsnEntry::new(record))),
            Ok(None) => None,
            Err(err) => {
                debug!("Error finding next USN entry: {err}");
                Some(Err(err))
            }
        }
    }
}
