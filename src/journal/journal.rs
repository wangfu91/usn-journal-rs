//! `UsnJournal` — high-level wrapper around the Windows USN journal FSCTLs.

use std::mem::size_of;

use log::{debug, warn};
use windows::Win32::Foundation::ERROR_JOURNAL_NOT_ACTIVE;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, DELETE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL,
    FSCTL_DELETE_USN_JOURNAL, FSCTL_QUERY_USN_JOURNAL, USN_DELETE_FLAG_DELETE,
    USN_DELETE_FLAG_NOTIFY, USN_DELETE_FLAGS, USN_JOURNAL_DATA_V0,
};

use crate::UsnResult;
use crate::volume::Volume;

use super::data::UsnJournalData;
use super::defaults::{
    DEFAULT_BUFFER_BYTES, DEFAULT_JOURNAL_ALLOCATION_DELTA, DEFAULT_JOURNAL_MAX_SIZE,
    USN_REASON_MASK_ALL,
};
use super::iter::UsnJournalIter;
use super::options::JournalIterOptions;

#[derive(Debug, Clone)]
/// Iterator for enumerating USN journal records on NTFS/ReFS volume.
///
/// This iterator yields `Result<UsnEntry, UsnError>` items, allowing applications
/// to handle individual entry errors without stopping the entire iteration process.
pub struct UsnJournal<'a> {
    pub(crate) volume: &'a Volume,
}

impl<'a> UsnJournal<'a> {
    /// Create a new `UsnJournal` instance.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        UsnJournal { volume }
    }

    /// Returns an iterator over the USN journal entries.
    ///
    /// The iterator yields `Result<UsnEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    ///
    /// This is fallible because it queries (and may create) the journal up
    /// front; subsequent per-record errors are surfaced as iterator items.
    pub fn try_iter(&self) -> UsnResult<UsnJournalIter> {
        let journal_data = self.query(true)?;
        Ok(UsnJournalIter {
            volume_handle: self.volume.handle,
            journal_id: journal_data.journal_id,
            buffer: vec![0u8; DEFAULT_BUFFER_BYTES],
            bytes_read: 0,
            offset: 0,
            next_start_usn: 0,
            reason_mask: USN_REASON_MASK_ALL,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 1,
        })
    }

    /// Returns an iterator over the USN journal entries with custom options.
    ///
    /// The iterator yields `Result<UsnEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    pub fn try_iter_with_options(
        &self,
        options: JournalIterOptions,
    ) -> UsnResult<UsnJournalIter> {
        let journal_data = self.query(true)?;
        Ok(UsnJournalIter {
            volume_handle: self.volume.handle,
            journal_id: journal_data.journal_id,
            buffer: vec![0u8; options.buffer_size],
            bytes_read: 0,
            offset: 0,
            next_start_usn: options.start_usn.get(),
            reason_mask: options.reason_mask,
            return_only_on_close: options.only_on_close as u32,
            timeout: options.timeout,
            bytes_to_wait_for: options.wait_for_more as u64,
        })
    }

    /// Query the USN journal state for a volume, optionally creating it if not active.
    ///
    /// # Arguments
    /// * `create_if_not_active` - If true, create the journal if it does not exist.
    ///
    /// # Returns
    /// * `Ok(UsnJournalData)` - The current journal state.
    /// * `Err(UsnError)` - If the query or creation fails.
    pub fn query(&self, create_if_not_active: bool) -> UsnResult<UsnJournalData> {
        match self.query_core() {
            Err(err) => {
                if err.code() == ERROR_JOURNAL_NOT_ACTIVE.into() && create_if_not_active {
                    self.create_or_update(
                        DEFAULT_JOURNAL_MAX_SIZE,
                        DEFAULT_JOURNAL_ALLOCATION_DELTA,
                    )?;

                    let journal_data = self.query_core()?;
                    Ok(journal_data.into())
                } else {
                    warn!("Error querying USN journal: {err}");
                    Err(err.into())
                }
            }
            Ok(journal_data) => {
                debug!("USN journal data: {journal_data:#?}");
                Ok(journal_data.into())
            }
        }
    }

    /// Core function to query the USN journal state.
    fn query_core(&self) -> std::result::Result<USN_JOURNAL_DATA_V0, windows::core::Error> {
        let journal_data = USN_JOURNAL_DATA_V0::default();
        let bytes_return = 0u32;

        // SAFETY: `self.volume.handle` is a live volume handle owned by
        // `self`. The output buffer points to a stack-allocated
        // `USN_JOURNAL_DATA_V0` whose size we pass exactly; `bytes_return`
        // is a stack `u32` we hand off as an out-pointer. The FSCTL
        // does not retain any of these pointers past the call.
        unsafe {
            // https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier
            // To obtain the identifier of the current change journal on a specified volume,
            // use the FSCTL_QUERY_USN_JOURNAL control code.
            //
            // To perform this and all other change journal operations,
            // you must have system administrator privileges.
            // That is, you must be a member of the Administrators group.
            DeviceIoControl(
                self.volume.handle,
                FSCTL_QUERY_USN_JOURNAL,
                None,
                0,
                Some(&journal_data as *const _ as *mut _),
                std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
                Some(&bytes_return as *const _ as *mut _),
                None,
            )
        }?;

        Ok(journal_data)
    }

    /// Create or update the USN journal on a volume.
    ///
    /// # Arguments
    /// * `max_size` - Maximum size of the journal in bytes.
    /// * `allocation_delta` - Allocation delta in bytes.
    ///
    /// # Returns
    /// * `Ok(())` on success, or `Err(UsnError)` on failure.
    pub fn create_or_update(&self, max_size: u64, allocation_delta: u64) -> UsnResult<()> {
        let create_data = CREATE_USN_JOURNAL_DATA {
            MaximumSize: max_size,
            AllocationDelta: allocation_delta,
        };

        // SAFETY: `self.volume.handle` is a live volume handle. The
        // input pointer references the stack-local `create_data` for the
        // duration of the call; we pass no output buffer.
        unsafe {
            // https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_create_usn_journal
            // FSCTL_CREATE_USN_JOURNAL
            // Creates an update sequence number (USN) change journal stream on a target volume, or modifies an existing change journal stream.
            DeviceIoControl(
                self.volume.handle,
                FSCTL_CREATE_USN_JOURNAL,
                Some(&create_data as *const _ as *mut _),
                size_of::<CREATE_USN_JOURNAL_DATA>() as u32,
                None,
                0,
                None,
                None,
            )
        }?;

        debug!("Created USN journal successfully.");

        Ok(())
    }

    /// Delete the USN journal from a volume.
    /// # Returns
    /// * `Ok(())` on success, or `Err(UsnError)` on failure.
    pub fn delete(&self) -> UsnResult<()> {
        let journal_data = self.query(false)?;
        let delete_flags: USN_DELETE_FLAGS = USN_DELETE_FLAG_DELETE | USN_DELETE_FLAG_NOTIFY;
        let delete_data = DELETE_USN_JOURNAL_DATA {
            UsnJournalID: journal_data.journal_id,
            DeleteFlags: delete_flags,
        };

        // SAFETY: `self.volume.handle` is a live volume handle. The
        // input pointer references the stack-local `delete_data` for the
        // duration of the call; we pass no output buffer.
        unsafe {
            DeviceIoControl(
                self.volume.handle,
                FSCTL_DELETE_USN_JOURNAL,
                Some(&delete_data as *const _ as *mut _),
                size_of::<DELETE_USN_JOURNAL_DATA>() as u32,
                None,
                0,
                None,
                None,
            )
        }?;

        debug!("Deleted USN journal successfully.");

        Ok(())
    }
}
