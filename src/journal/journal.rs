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
use crate::errors::UsnError;
use crate::volume::Volume;

use super::data::UsnJournalData;
use super::defaults::{DEFAULT_JOURNAL_ALLOCATION_DELTA, DEFAULT_JOURNAL_MAX_SIZE};
use super::iter::{UsnJournalIter, UsnJournalIterConfig};
use super::options::JournalIterOptions;

#[derive(Debug, Clone)]
/// Iterator for enumerating USN journal records on NTFS/ReFS volume.
///
/// This iterator yields `Result<UsnEntry, UsnError>` items, allowing applications
/// to handle individual entry errors without stopping the entire iteration process.
pub struct UsnJournal<'a> {
    /// Volume whose USN journal will be queried.
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
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn try_iter(&self) -> UsnResult<UsnJournalIter> {
        self.try_iter_with_options(JournalIterOptions::default())
    }

    /// Iterate over available records using default options.
    pub fn iter(&self) -> UsnResult<UsnJournalIter> {
        self.try_iter()
    }

    /// Iterate with validated options. Alias for [Self::try_iter_with_options].
    pub fn iter_with_options(&self, options: JournalIterOptions) -> UsnResult<UsnJournalIter> {
        self.try_iter_with_options(options)
    }

    /// Returns an iterator over the USN journal entries with custom options.
    ///
    /// The iterator yields `Result<UsnEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn try_iter_with_options(&self, options: JournalIterOptions) -> UsnResult<UsnJournalIter> {
        self.try_iter_with_buffer(options, Vec::new())
    }

    /// Construct an iterator using a caller-owned reusable buffer.
    /// Resizes the buffer to the configured length before the first read.
    pub fn try_iter_with_buffer(
        &self,
        options: JournalIterOptions,
        mut buffer: Vec<u8>,
    ) -> UsnResult<UsnJournalIter> {
        let journal_data = self.query_or_create()?;
        buffer.resize(options.buffer_bytes.get(), 0);
        Ok(UsnJournalIter::new(
            self.volume.shared_handle(),
            journal_data.journal_id,
            buffer,
            UsnJournalIterConfig {
                next_start_usn: options.start_usn.get(),
                reason_mask: options.reason_mask.bits(),
                return_only_on_close: options.only_on_close as u32,
                timeout: options.timeout_secs,
                bytes_to_wait_for: options.wait_for_more as u64,
            },
        ))
    }

    /// Query the current USN journal state for the volume.
    ///
    /// # Errors
    ///
    /// Returns [`UsnError::JournalNotActive`] if the volume has no active change
    /// journal. Use [`Self::query_or_create`] to create one on demand instead.
    pub fn query(&self) -> UsnResult<UsnJournalData> {
        match self.query_core() {
            Ok(journal_data) => {
                debug!("USN journal data: {journal_data:#?}");
                Ok(journal_data.into())
            }
            Err(err) if err.code() == ERROR_JOURNAL_NOT_ACTIVE.into() => {
                Err(UsnError::JournalNotActive)
            }
            Err(err) => {
                warn!("Error querying USN journal: {err}");
                Err(err.into())
            }
        }
    }

    /// Query the USN journal state, creating the journal first if it is not active.
    ///
    /// Equivalent to [`Self::query`] but transparently creates a default journal
    /// (see [`Self::create_or_update`]) when the volume has none, then re-queries.
    pub fn query_or_create(&self) -> UsnResult<UsnJournalData> {
        match self.query() {
            Err(UsnError::JournalNotActive) => {
                self.create_or_update(DEFAULT_JOURNAL_MAX_SIZE, DEFAULT_JOURNAL_ALLOCATION_DELTA)?;
                let journal_data = self.query_core()?;
                Ok(journal_data.into())
            }
            other => other,
        }
    }

    /// Core function to query the USN journal state.
    fn query_core(&self) -> std::result::Result<USN_JOURNAL_DATA_V0, windows::core::Error> {
        let mut journal_data = USN_JOURNAL_DATA_V0::default();
        let mut bytes_return = 0u32;

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
                self.volume.handle(),
                FSCTL_QUERY_USN_JOURNAL,
                None,
                0,
                Some(&mut journal_data as *mut _ as *mut _),
                std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
                Some(&mut bytes_return),
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
                self.volume.handle(),
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
        let journal_data = self.query()?;
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
                self.volume.handle(),
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

impl Volume {
    /// Create a [`UsnJournal`] reader for this volume.
    ///
    /// Convenience for [`UsnJournal::new`], so you can write
    /// `volume.journal().try_iter()?` without importing [`UsnJournal`].
    #[must_use]
    pub fn journal(&self) -> UsnJournal<'_> {
        UsnJournal::new(self)
    }
}
