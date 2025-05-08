use log::warn;
use windows::Win32::{
    Foundation::{ERROR_HANDLE_EOF, HANDLE},
    System::{
        IO::DeviceIoControl,
        Ioctl::{self, USN_RECORD_V2},
    },
};

use crate::{DEFAULT_BUFFER_SIZE, Usn, usn_entry::UsnEntry};

pub struct Mft {
    volume_handle: HANDLE,
    buffer: Vec<u8>,
    bytes_read: u32,
    offset: u32,
    next_start_fid: u64,
    low_usn: Usn,
    high_usn: Usn,
}

pub struct MftEnumOptions {
    pub low_usn: Usn,
    pub high_usn: Usn,
    pub buffer_size: usize,
}

impl Default for MftEnumOptions {
    fn default() -> Self {
        MftEnumOptions {
            low_usn: 0,
            high_usn: i64::MAX,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

impl Mft {
    pub fn new(volume_handle: HANDLE) -> Self {
        Mft {
            volume_handle,
            buffer: vec![0u8; DEFAULT_BUFFER_SIZE],
            bytes_read: 0,
            offset: 0,
            next_start_fid: 0,
            low_usn: 0,
            high_usn: i64::MAX,
        }
    }

    pub fn new_with_options(volume_handle: HANDLE, options: MftEnumOptions) -> Self {
        Mft {
            volume_handle,
            buffer: vec![0u8; options.buffer_size],
            bytes_read: 0,
            offset: 0,
            next_start_fid: options.low_usn as u64,
            low_usn: options.low_usn,
            high_usn: options.high_usn,
        }
    }

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

            warn!("Error reading MFT data: {}", err);
            return Err(err.into());
        }

        Ok(true)
    }

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

impl Iterator for Mft {
    type Item = UsnEntry;

    fn next(&mut self) -> Option<Self::Item> {
        match self.find_next_entry() {
            Ok(Some(record)) => Some(UsnEntry::new(record)),
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
    use crate::{
        tests_utils::{setup, teardown},
        utils,
    };

    use super::*;

    #[test]
    fn mft_iter_test() -> anyhow::Result<()> {
        // Setup the test environment
        let (mount_point, uuid) = setup()?;

        let result = {
            let volume_handle = utils::get_volume_handle_from_mount_point(mount_point.as_path())?;
            let mft = Mft::new(volume_handle);
            for entry in mft {
                println!("MFT entry: {:?}", entry);
                // Check if the USN entry is valid
                assert!(entry.usn >= 0, "USN is not valid");
                assert!(entry.fid > 0, "File ID is not valid");
                assert!(!entry.file_name.is_empty(), "File name is not valid");
                assert!(entry.parent_fid > 0, "Parent File ID is not valid");
                assert!(entry.reason == 0, "Reason is not valid");
                assert!(entry.file_attributes.0 > 0, "File attributes are not valid");
            }

            Ok(())
        };

        // Teardown the test environment
        teardown(uuid)?;

        // Return the result of the test
        result
    }
}
