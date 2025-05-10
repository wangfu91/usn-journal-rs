//! Represents the Master File Table (MFT) enumerator for a given NTFS volume.
//!
//! The `Mft` struct provides an iterator interface to enumerate USN (Update Sequence Number) records
//! from the MFT using the Windows FSCTL_ENUM_USN_DATA control code. It manages the buffer and state
//! required to sequentially retrieve and parse USN records from the volume.
//!
//! # Example: Enumerating MFT Entries
//! ```rust
//! use usn_journal_rs::mft::Mft;
//!
//! let drive_letter = 'C';
//! let mft = Mft::new_from_drive_letter(drive_letter).unwrap();
//! for entry in mft.iter().take(10) {
//!     println!("MFT entry: {:?}", entry);
//! }
//! ```
//!
//! # Errors
//! Errors encountered during enumeration are logged and cause the iterator to end.

use crate::{utils, Usn, DEFAULT_BUFFER_SIZE};
use log::warn;
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::Path};
use windows::Win32::{
    Foundation::{ERROR_HANDLE_EOF, HANDLE},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
    },
    System::{
        Ioctl::{self, USN_RECORD_V2},
        IO::DeviceIoControl,
    },
};

/// Represents a single entry in the Master File Table (MFT).
#[derive(Debug)]
pub struct MftEntry {
    pub usn: Usn,
    pub fid: u64,
    pub parent_fid: u64,
    pub file_name: OsString,
    pub file_attributes: u32,
}

impl MftEntry {
    /// Creates a new `MftEntry` from a raw USN_RECORD_V2 record.
    pub(crate) fn new(record: &USN_RECORD_V2) -> Self {
        let file_name_len = record.FileNameLength as usize / std::mem::size_of::<u16>();
        let file_name_data =
            unsafe { std::slice::from_raw_parts(record.FileName.as_ptr(), file_name_len) };
        let file_name = OsString::from_wide(file_name_data);

        MftEntry {
            usn: record.Usn,
            fid: record.FileReferenceNumber,
            parent_fid: record.ParentFileReferenceNumber,
            file_name,
            file_attributes: record.FileAttributes,
        }
    }

    /// Returns true if this entry represents a directory.
    pub fn is_dir(&self) -> bool {
        let attributes = FILE_FLAGS_AND_ATTRIBUTES(self.file_attributes);
        attributes.contains(FILE_ATTRIBUTE_DIRECTORY)
    }

    /// Returns true if this entry represents a hidden file or directory.
    pub fn is_hidden(&self) -> bool {
        let attributes = FILE_FLAGS_AND_ATTRIBUTES(self.file_attributes);
        attributes.contains(FILE_ATTRIBUTE_HIDDEN)
    }
}

/// Options for enumerating the Master File Table (MFT).
///
/// Allows customization of the USN range and buffer size for enumeration.
pub struct EnumOptions {
    pub low_usn: Usn,
    pub high_usn: Usn,
    pub buffer_size: usize,
}

impl Default for EnumOptions {
    fn default() -> Self {
        EnumOptions {
            low_usn: 0,
            high_usn: i64::MAX,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

/// Represents the Master File Table (MFT) enumerator.
pub struct Mft {
    pub(crate) volume_handle: HANDLE,
    pub(crate) drive_letter: Option<char>,
}

impl Mft {
    /// Creates a new `Mft` instance with the given drive letter.
    pub fn new_from_drive_letter(drive_letter: char) -> anyhow::Result<Self> {
        let volume_handle = utils::get_volume_handle(drive_letter)?;
        Ok(Mft {
            volume_handle,
            drive_letter: Some(drive_letter),
        })
    }

    /// Creates a new `Mft` instance with the given volume mount point.
    pub fn new_from_mount_point(mount_point: &Path) -> anyhow::Result<Self> {
        let volume_handle = utils::get_volume_handle_from_mount_point(mount_point)?;
        Ok(Mft {
            volume_handle,
            drive_letter: None,
        })
    }

    /// Returns an iterator over the MFT entries.
    pub fn iter(&self) -> MftIter<'_> {
        MftIter {
            mft: self,
            low_usn: 0,
            high_usn: i64::MAX,
            buffer: vec![0u8; DEFAULT_BUFFER_SIZE],
            bytes_read: 0,
            offset: 0,
            next_start_fid: 0,
        }
    }

    /// Returns an iterator over the MFT entries with custom options.
    pub fn iter_with_options(&self, options: EnumOptions) -> MftIter<'_> {
        MftIter {
            mft: self,
            low_usn: options.low_usn,
            high_usn: options.high_usn,
            buffer: vec![0u8; options.buffer_size],
            bytes_read: 0,
            offset: 0,
            next_start_fid: options.low_usn as u64,
        }
    }
}

/// Iterator over MFT entries.
pub struct MftIter<'a> {
    mft: &'a Mft,
    low_usn: Usn,
    high_usn: Usn,
    buffer: Vec<u8>,
    bytes_read: u32,
    offset: u32,
    next_start_fid: u64,
}

impl MftIter<'_> {
    /// Reads the next chunk of MFT data into the buffer.
    ///
    /// Returns `Ok(true)` if data was read, `Ok(false)` if EOF, or an error.
    fn get_data(&mut self) -> anyhow::Result<bool> {
        // To enumerate files on a volume, use the FSCTL_ENUM_USN_DATA operation one or more times.
        // On the first call, set the starting point, the StartFileReferenceNumber member of the MFT_ENUM_DATA structure, to (DWORDLONG)0.
        let mft_enum_data = Ioctl::MFT_ENUM_DATA_V0 {
            StartFileReferenceNumber: self.next_start_fid,
            LowUsn: self.low_usn,
            HighUsn: self.high_usn,
        };

        if let Err(err) = unsafe {
            DeviceIoControl(
                self.mft.volume_handle,
                Ioctl::FSCTL_ENUM_USN_DATA,
                Some(&mft_enum_data as *const _ as _),
                size_of::<Ioctl::MFT_ENUM_DATA_V0>() as u32,
                Some(self.buffer.as_mut_ptr() as _),
                self.buffer.len() as u32,
                Some(&mut self.bytes_read),
                None,
            )
        } {
            if err.code() == ERROR_HANDLE_EOF.into() {
                return Ok(false);
            }

            warn!("Error reading MFT data: {}", err);
            return Err(err.into());
        }

        Ok(true)
    }

    /// Finds the next USN record in the buffer, reading more data if needed.
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
            // Each call to FSCTL_ENUM_USN_DATA retrieves the starting point for the subsequent call as the first entry in the output buffer.
            self.next_start_fid = unsafe { std::ptr::read(self.buffer.as_ptr() as *const u64) };
            self.offset = size_of::<u64>() as u32;
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

impl Iterator for MftIter<'_> {
    type Item = MftEntry;

    fn next(&mut self) -> Option<Self::Item> {
        match self.find_next_entry() {
            Ok(Some(record)) => Some(MftEntry::new(record)),
            Ok(None) => None,
            Err(err) => {
                warn!("Error finding next MFT entry: {}", err);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{setup, teardown};

    use super::*;

    #[test]
    fn mft_iter_test() -> anyhow::Result<()> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let mft = Mft::new_from_mount_point(mount_point.as_path())?;
            for entry in mft.iter() {
                println!("MFT entry: {:?}", entry);
                // Check if the Mft entry is valid
                assert!(entry.usn >= 0, "USN is not valid");
                assert!(entry.fid > 0, "File ID is not valid");
                assert!(!entry.file_name.is_empty(), "File name is not valid");
                assert!(entry.parent_fid > 0, "Parent File ID is not valid");
                assert!(entry.file_attributes > 0, "File attributes are not valid");
            }

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }
}
