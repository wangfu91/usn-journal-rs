//! USN Journal module: provides access to the Windows NTFS Update Sequence Number (USN) change journal.
//!
//! This module enables querying, creating, deleting, and iterating over the USN change journal on NTFS volumes.
//! It provides safe Rust abstractions over the Windows API for monitoring file system changes efficiently.

use std::{ffi::c_void, mem::size_of};

use anyhow::Context;
use log::{debug, warn};
use windows::Win32::{
    Foundation::{ERROR_HANDLE_EOF, ERROR_JOURNAL_NOT_ACTIVE, HANDLE},
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            CREATE_USN_JOURNAL_DATA, DELETE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL,
            FSCTL_DELETE_USN_JOURNAL, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL,
            READ_USN_JOURNAL_DATA_V0, USN_DELETE_FLAG_DELETE, USN_DELETE_FLAG_NOTIFY,
            USN_DELETE_FLAGS, USN_JOURNAL_DATA_V0, USN_RECORD_V2,
        },
    },
};

use crate::{
    DEFAULT_BUFFER_SIZE, DEFAULT_JOURNAL_ALLOCATION_DELTA, DEFAULT_JOURNAL_MAX_SIZE,
    USN_REASON_MASK_ALL, Usn, usn_entry::UsnEntry,
};

#[derive(Debug, Clone)]
/// Options for enumerating the USN journal.
///
/// Allows customization of the starting USN, reason mask, buffer size, and other parameters.
pub struct EnumOptions {
    pub start_usn: Usn,
    pub reason_mask: u32,
    pub only_on_close: bool,
    pub timeout: u64,
    pub wait_for_more: bool,
    pub buffer_size: usize,
}

impl Default for EnumOptions {
    fn default() -> Self {
        EnumOptions {
            start_usn: 0,
            reason_mask: USN_REASON_MASK_ALL,
            only_on_close: false,
            timeout: 0,
            wait_for_more: false,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

#[derive(Debug, Clone)]
/// Iterator for enumerating USN journal records on NTFS/ReFS volume.
pub struct UsnJournal {
    pub(crate) volume_handle: HANDLE,
    journal_id: u64,
    pub(crate) drive_letter: Option<char>,
    pub next_usn: Usn,
}

impl UsnJournal {
    /// Create a new `UsnJournal` instance with the given drive letter.
    pub fn new_from_drive_letter(drive_letter: char) -> anyhow::Result<Self> {
        let volume_handle = crate::utils::get_volume_handle(drive_letter)?;
        let journal_data = query(volume_handle, true)?;
        Ok(UsnJournal {
            volume_handle,
            journal_id: journal_data.UsnJournalID,
            drive_letter: Some(drive_letter),
            next_usn: journal_data.NextUsn,
        })
    }

    /// Create a new `UsnJournal` instance with the given volume mount point.
    pub fn new_from_mount_point(mount_point: &std::path::Path) -> anyhow::Result<Self> {
        let volume_handle = crate::utils::get_volume_handle_from_mount_point(mount_point)?;
        let journal_data = query(volume_handle, true)?;
        Ok(UsnJournal {
            volume_handle,
            journal_id: journal_data.UsnJournalID,
            drive_letter: None,
            next_usn: journal_data.NextUsn,
        })
    }

    /// Returns an iterator over the USN journal entries.
    pub fn iter(&self) -> UsnJournalIter<'_> {
        UsnJournalIter {
            usn_journal: self,
            buffer: vec![0u8; DEFAULT_BUFFER_SIZE],
            bytes_read: 0,
            offset: 0,
            next_start_usn: 0,
            reason_mask: USN_REASON_MASK_ALL,
            return_only_on_close: 0,
            timeout: 0,
            bytes_to_wait_for: 1,
        }
    }

    pub fn iter_with_options(&self, options: EnumOptions) -> UsnJournalIter<'_> {
        UsnJournalIter {
            usn_journal: self,
            buffer: vec![0u8; options.buffer_size],
            bytes_read: 0,
            offset: 0,
            next_start_usn: options.start_usn,
            reason_mask: options.reason_mask,
            return_only_on_close: options.only_on_close as u32,
            timeout: options.timeout,
            bytes_to_wait_for: options.wait_for_more as u64,
        }
    }
}

/// Iterate over USN journal entries.
pub struct UsnJournalIter<'a> {
    usn_journal: &'a UsnJournal,
    buffer: Vec<u8>,
    bytes_read: u32,
    offset: u32,
    next_start_usn: Usn,
    reason_mask: u32,
    return_only_on_close: u32,
    timeout: u64,
    bytes_to_wait_for: u64,
}

impl UsnJournalIter<'_> {
    /// Read the next chunk of USN journal data into the buffer.
    ///
    /// Returns `Ok(true)` if data was read, `Ok(false)` if EOF, or an error.
    fn get_data(&mut self) -> anyhow::Result<bool> {
        let read_data = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: self.next_start_usn,
            ReasonMask: self.reason_mask,
            ReturnOnlyOnClose: self.return_only_on_close,
            Timeout: self.timeout,
            BytesToWaitFor: self.bytes_to_wait_for,
            UsnJournalID: self.usn_journal.journal_id,
        };

        if let Err(err) = unsafe {
            DeviceIoControl(
                self.usn_journal.volume_handle,
                FSCTL_READ_USN_JOURNAL,
                Some(&read_data as *const _ as *mut _),
                size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                Some(self.buffer.as_mut_ptr() as *mut c_void),
                self.buffer.len() as u32,
                Some(&mut self.bytes_read),
                None,
            )
        } {
            if err.code() == ERROR_HANDLE_EOF.into() {
                return Ok(false);
            }

            warn!("Error reading USN data: {}", err);
            return Err(err.into());
        }

        Ok(true)
    }

    /// Find the next USN record in the buffer, reading more data if needed.
    ///
    /// Returns `Ok(Some(&USN_RECORD_V2))` if a record is found, `Ok(None)` if EOF, or an error.
    fn find_next_entry(&mut self) -> anyhow::Result<Option<&USN_RECORD_V2>> {
        if self.offset < self.bytes_read {
            let record = unsafe {
                &*(self.buffer.as_ptr().offset(self.offset as isize) as *const USN_RECORD_V2)
            };
            self.offset += record.RecordLength;
            return Ok(Some(record));
        }

        // We need to read more data
        if self.get_data()? {
            // https://learn.microsoft.com/en-us/windows/win32/fileio/walking-a-buffer-of-change-journal-records
            // The USN returned as the first item in the output buffer is the USN of the next record number to be retrieved.
            // Use this value to continue reading records from the end boundary forward.
            self.next_start_usn = unsafe { std::ptr::read(self.buffer.as_ptr() as *const Usn) };
            self.offset = std::mem::size_of::<Usn>() as u32;

            if self.offset < self.bytes_read {
                let record = unsafe {
                    &*(self.buffer.as_ptr().offset(self.offset as isize) as *const USN_RECORD_V2)
                };
                self.offset += record.RecordLength;
                return Ok(Some(record));
            }
        }

        // EOF, no more data to read
        Ok(None)
    }
}

impl Iterator for UsnJournalIter<'_> {
    type Item = UsnEntry;

    fn next(&mut self) -> Option<Self::Item> {
        match self.find_next_entry() {
            Ok(Some(record)) => Some(UsnEntry::new(record)),
            Ok(None) => None,
            Err(err) => {
                warn!("Error finding next USN entry: {}", err);
                None
            }
        }
    }
}

/// Query the USN journal state for a volume, optionally creating it if not active.
///
/// # Arguments
/// * `volume_handle` - Handle to the NTFS volume.
/// * `create_if_not_active` - If true, create the journal if it does not exist.
///
/// # Returns
/// * `Ok(USN_JOURNAL_DATA_V0)` - The current journal state.
/// * `Err(anyhow::Error)` - If the query or creation fails.
pub fn query(
    volume_handle: HANDLE,
    create_if_not_active: bool,
) -> anyhow::Result<USN_JOURNAL_DATA_V0> {
    match query_core(volume_handle) {
        Err(err) => {
            if err.code() == ERROR_JOURNAL_NOT_ACTIVE.into() && create_if_not_active {
                create_or_update(
                    volume_handle,
                    DEFAULT_JOURNAL_MAX_SIZE,
                    DEFAULT_JOURNAL_ALLOCATION_DELTA,
                )
                .context("Failed to create USN journal")?;

                let journal_data =
                    query_core(volume_handle).context("Failed to query USN journal")?;
                Ok(journal_data)
            } else {
                warn!("Error querying USN journal: {}", err);
                Err(err.into())
            }
        }
        Ok(journal_data) => {
            debug!("USN journal data: {:#?}", journal_data);
            Ok(journal_data)
        }
    }
}

fn query_core(volume_handle: HANDLE) -> Result<USN_JOURNAL_DATA_V0, windows::core::Error> {
    let journal_data = USN_JOURNAL_DATA_V0::default();
    let bytes_return = 0u32;

    unsafe {
        // https://learn.microsoft.com/en-us/windows/win32/fileio/using-the-change-journal-identifier
        // To obtain the identifier of the current change journal on a specified volume,
        // use the FSCTL_QUERY_USN_JOURNAL control code.
        // To perform this and all other change journal operations,
        // you must have system administrator privileges.
        // That is, you must be a member of the Administrators group.
        DeviceIoControl(
            volume_handle,
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
/// * `volume_handle` - Handle to the NTFS volume.
/// * `max_size` - Maximum size of the journal in bytes.
/// * `allocation_delta` - Allocation delta in bytes.
///
/// # Returns
/// * `Ok(())` on success, or `Err(anyhow::Error)` on failure.
pub fn create_or_update(
    volume_handle: HANDLE,
    max_size: u64,
    allocation_delta: u64,
) -> anyhow::Result<()> {
    let create_data = CREATE_USN_JOURNAL_DATA {
        MaximumSize: max_size,
        AllocationDelta: allocation_delta,
    };

    unsafe {
        // https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_create_usn_journal
        // FSCTL_CREATE_USN_JOURNAL
        // Creates an update sequence number (USN) change journal stream on a target volume, or modifies an existing change journal stream.
        DeviceIoControl(
            volume_handle,
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
///
/// # Arguments
/// * `volume_handle` - Handle to the NTFS volume.
/// * `journal_id` - The USN journal identifier.
///
/// # Returns
/// * `Ok(())` on success, or `Err(anyhow::Error)` on failure.
pub fn delete(volume_handle: HANDLE, journal_id: u64) -> anyhow::Result<()> {
    let delete_flags: USN_DELETE_FLAGS = USN_DELETE_FLAG_DELETE | USN_DELETE_FLAG_NOTIFY;
    let delete_data = DELETE_USN_JOURNAL_DATA {
        UsnJournalID: journal_id,
        DeleteFlags: delete_flags,
    };

    unsafe {
        DeviceIoControl(
            volume_handle,
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

#[cfg(test)]
mod tests {
    use crate::{
        DEFAULT_JOURNAL_ALLOCATION_DELTA, DEFAULT_JOURNAL_MAX_SIZE,
        tests_utils::{setup, teardown},
        utils,
    };
    use anyhow::Ok;

    #[test]
    fn query_usn_journal_test() -> anyhow::Result<()> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let volume_handle = utils::get_volume_handle_from_mount_point(mount_point.as_path())?;
            let journal_data = super::query(volume_handle, true)?;
            println!("USN journal data: {:?}", journal_data);

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }

    #[test]
    fn delete_usn_journal_test() -> anyhow::Result<()> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let volume_handle = utils::get_volume_handle_from_mount_point(mount_point.as_path())?;
            let journal_data = super::query(volume_handle, true)?;
            println!("USN journal data: {:?}", journal_data);
            super::delete(volume_handle, journal_data.UsnJournalID)?;

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }

    #[test]
    fn create_usn_journal_test() -> anyhow::Result<()> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let volume_handle = utils::get_volume_handle_from_mount_point(mount_point.as_path())?;
            let journal_data = super::query(volume_handle, true)?;
            println!("USN journal data: {:?}", journal_data);
            super::create_or_update(
                volume_handle,
                DEFAULT_JOURNAL_MAX_SIZE,
                DEFAULT_JOURNAL_ALLOCATION_DELTA,
            )?;

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }

    #[test]
    fn usn_journal_iter_test() -> anyhow::Result<()> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let usn_journal = super::UsnJournal::new_from_mount_point(&mount_point)?;
            let mut previous_usn = -1i64;
            for entry in usn_journal.iter() {
                println!("USN entry: {:?}", entry);
                // Check if the USN entry is valid
                assert!(entry.usn >= 0, "USN is not valid");
                assert!(entry.usn > previous_usn, "USN entries are not in order");
                assert!(entry.fid > 0, "File ID is not valid");
                assert!(!entry.file_name.is_empty(), "File name is not valid");
                assert!(entry.parent_fid > 0, "Parent File ID is not valid");
                assert!(entry.reason > 0, "Reason is not valid");
                assert!(entry.file_attributes.0 > 0, "File attributes are not valid");
                assert!(entry.time > std::time::UNIX_EPOCH, "Time is not valid");

                previous_usn = entry.usn;
            }

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }
}
