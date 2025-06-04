//! Represents the Master File Table (MFT) enumerator for a given NTFS volume.
//!
//! The `Mft` struct provides an iterator interface to enumerate USN records
//! from the MFT using the Windows FSCTL_ENUM_USN_DATA control code. It manages the buffer and state
//! required to sequentially retrieve and parse USN records from the volume.

use crate::{DEFAULT_BUFFER_SIZE, Usn, errors::UsnError, volume::Volume};
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::Path};
use windows::Win32::{
    Foundation::{ERROR_HANDLE_EOF, HANDLE},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{self, USN_RECORD_V2},
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

    pub fn pretty_format<P>(&self, full_path_opt: Option<P>) -> String
    where
        P: AsRef<Path>,
    {
        let mut output = String::new();
        output.push_str(&format!("{:<20}: 0x{:x}\n", "File ID", self.fid));
        output.push_str(&format!(
            "{:<20}: 0x{:x}\n",
            "Parent File ID", self.parent_fid
        ));
        output.push_str(&format!(
            "{:<20}: {}\n",
            "Type",
            if self.is_dir() { "Directory" } else { "File" }
        ));
        if let Some(full_path) = full_path_opt {
            output.push_str(&format!(
                "{:<20}: {}\n",
                "Path",
                full_path.as_ref().to_string_lossy()
            ));
        } else {
            // Fallback to file name if full path is not available
            output.push_str(&format!(
                "{:<20}: {}\n",
                "Path",
                self.file_name.to_string_lossy()
            ));
        }
        output
    }
}

/// Options for enumerating the Master File Table (MFT).
///
/// Allows customization of the USN range and buffer size for enumeration.
#[derive(Debug)]
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
#[derive(Debug)]
pub struct Mft<'a> {
    pub(crate) volume: &'a Volume,
}

impl<'a> Mft<'a> {
    /// Creates a new `Mft` instance.
    pub fn new(volume: &'a Volume) -> Self {
        Mft { volume }
    }

    /// Returns an iterator over the MFT entries.
    pub fn iter(&self) -> MftIter {
        MftIter {
            volume_handle: self.volume.handle,
            low_usn: 0,
            high_usn: i64::MAX,
            buffer: vec![0u8; DEFAULT_BUFFER_SIZE],
            bytes_read: 0,
            offset: 0,
            next_start_fid: 0,
        }
    }

    /// Returns an iterator over the MFT entries with custom enumerate options.
    pub fn iter_with_options(&self, options: EnumOptions) -> MftIter {
        MftIter {
            volume_handle: self.volume.handle,
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
pub struct MftIter {
    volume_handle: HANDLE,
    low_usn: Usn,
    high_usn: Usn,
    buffer: Vec<u8>,
    bytes_read: u32,
    offset: u32,
    next_start_fid: u64,
}

impl MftIter {
    /// Reads the next chunk of MFT data into the buffer.
    ///
    /// Returns `Ok(true)` if data was read, `Ok(false)` if EOF, or an error.
    fn get_data(&mut self) -> Result<bool, UsnError> {
        // To enumerate files on a volume, use the FSCTL_ENUM_USN_DATA operation one or more times.
        // On the first call, set the starting point, the StartFileReferenceNumber member of the MFT_ENUM_DATA structure, to (DWORDLONG)0.
        let mft_enum_data = Ioctl::MFT_ENUM_DATA_V0 {
            StartFileReferenceNumber: self.next_start_fid,
            LowUsn: self.low_usn,
            HighUsn: self.high_usn,
        };

        if let Err(err) = unsafe {
            DeviceIoControl(
                self.volume_handle,
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
            return Err(UsnError::WinApiError(err));
        }
        Ok(true)
    }

    /// Finds the next USN record in the buffer, reading more data if needed.
    ///
    /// Returns `Ok(Some(&USN_RECORD_V2))` if a record is found, `Ok(None)` if EOF, or an error.
    fn find_next_entry(&mut self) -> Result<Option<&USN_RECORD_V2>, UsnError> {
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

impl Iterator for MftIter {
    type Item = MftEntry;

    fn next(&mut self) -> Option<Self::Item> {
        match self.find_next_entry() {
            Ok(Some(record)) => Some(MftEntry::new(record)),
            Ok(None) => None,
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::{setup, teardown};

    use super::*;

    #[test]
    fn mft_iter_test() -> Result<(), super::UsnError> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let volume = Volume::from_mount_point(mount_point.as_path())?;
            let mft = Mft::new(&volume);
            for entry in mft.iter() {
                println!("MFT entry: {:?}", entry);
                // Check if the Mft entry is valid
                assert!(entry.usn >= 0, "USN is not valid");
                assert!(entry.fid > 0, "File ID is not valid");
                assert!(!entry.file_name.is_empty(), "File name is not valid");
                assert!(entry.parent_fid > 0, "Parent File ID is not valid");
                assert!(
                    entry.file_attributes > 0,
                    "File attributes are not valid (zero is allowed if no special flags are set)"
                );
            }

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }
}
