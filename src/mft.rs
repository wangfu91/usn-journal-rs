//! Represents the Master File Table (MFT) enumerator for a given NTFS volume.
//!
//! The `Mft` struct provides an iterator interface to enumerate USN records
//! from the MFT using the Windows FSCTL_ENUM_USN_DATA control code. It manages the buffer and state
//! required to sequentially retrieve and parse USN records from the volume.

use crate::{DEFAULT_BUFFER_SIZE, Usn, UsnResult, errors::UsnError, volume::Volume};
use log::debug;
use std::{ffi::OsString, mem::size_of, os::windows::ffi::OsStringExt, path::Path};
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
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
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
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
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
///
/// This iterator yields `Result<MftEntry, UsnError>` items, allowing applications
/// to handle individual entry errors without stopping the entire iteration process.
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
mod tests {
    use super::*;
    use injectorpp::interface::injector::*;
    use std::mem::offset_of;
    use windows::Win32::{
        Foundation::{ERROR_INVALID_HANDLE, HANDLE},
        System::{IO::DeviceIoControl, Ioctl::USN_RECORD_V2},
    };

    // Test data generators
    #[allow(clippy::too_many_arguments)]
    fn create_mock_usn_record(
        usn: i64,
        file_id: u64,
        parent_file_id: u64,
        file_name: &str,
        file_attributes: u32,
    ) -> Vec<u8> {
        let file_name_utf16: Vec<u16> = file_name.encode_utf16().collect();
        let file_name_len = file_name_utf16.len() * 2; // length in bytes
        let filename_offset = offset_of!(USN_RECORD_V2, FileName);
        let record_len = filename_offset + file_name_len;

        // Create a properly aligned buffer
        let mut buffer = vec![0u8; record_len];

        // Create the USN_RECORD_V2 structure (without the variable-length filename)
        let base_record = USN_RECORD_V2 {
            RecordLength: record_len as u32,
            MajorVersion: 2,
            MinorVersion: 0,
            FileReferenceNumber: file_id,
            ParentFileReferenceNumber: parent_file_id,
            Usn: usn,
            TimeStamp: 0,
            Reason: 0,
            SourceInfo: 0,
            SecurityId: 0,
            FileAttributes: file_attributes,
            FileNameLength: file_name_len as u16,
            FileNameOffset: filename_offset as u16,
            FileName: [0; 1], // This will be overwritten
        };

        // Copy the base structure to the buffer
        unsafe {
            std::ptr::copy_nonoverlapping(
                &base_record as *const USN_RECORD_V2 as *const u8,
                buffer.as_mut_ptr(),
                filename_offset,
            );
        }

        // Copy the filename data
        unsafe {
            std::ptr::copy_nonoverlapping(
                file_name_utf16.as_ptr() as *const u8,
                buffer.as_mut_ptr().add(filename_offset),
                file_name_len,
            );
        }

        buffer
    }

    // Unit tests for MftEntry
    mod mft_entry_tests {
        use super::*;

        #[test]
        fn test_mft_entry_new_basic() {
            let record_data = create_mock_usn_record(
                100,   // usn
                12345, // file_id
                67890, // parent_file_id
                "test.txt", 0x20, // FILE_ATTRIBUTE_ARCHIVE
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert_eq!(entry.usn, 100);
            assert_eq!(entry.fid, 12345);
            assert_eq!(entry.parent_fid, 67890);
            assert_eq!(entry.file_name.to_string_lossy(), "test.txt");
            assert_eq!(entry.file_attributes, 0x20);
        }

        #[test]
        fn test_mft_entry_is_dir_true() {
            let record_data = create_mock_usn_record(
                100, 12345, 67890, "folder", 0x10, // FILE_ATTRIBUTE_DIRECTORY
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert!(entry.is_dir());
            assert!(!entry.is_hidden());
        }

        #[test]
        fn test_mft_entry_is_dir_false() {
            let record_data = create_mock_usn_record(
                100, 12345, 67890, "file.txt", 0x20, // FILE_ATTRIBUTE_ARCHIVE
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert!(!entry.is_dir());
            assert!(!entry.is_hidden());
        }

        #[test]
        fn test_mft_entry_is_hidden_true() {
            let record_data = create_mock_usn_record(
                100, 12345, 67890, ".hidden", 0x02, // FILE_ATTRIBUTE_HIDDEN
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert!(entry.is_hidden());
            assert!(!entry.is_dir());
        }

        #[test]
        fn test_mft_entry_combined_attributes() {
            let record_data = create_mock_usn_record(
                100,
                12345,
                67890,
                ".hidden_folder",
                0x12, // FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_HIDDEN
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert!(entry.is_dir());
            assert!(entry.is_hidden());
        }

        #[test]
        fn test_mft_entry_unicode_filename() {
            let record_data = create_mock_usn_record(
                100,
                12345,
                67890,
                "测试文件.txt", // Chinese characters
                0x20,
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert_eq!(entry.file_name.to_string_lossy(), "测试文件.txt");
        }

        #[test]
        fn test_mft_entry_empty_filename() {
            let record_data = create_mock_usn_record(100, 12345, 67890, "", 0x20);

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            assert!(entry.file_name.is_empty());
        }

        #[test]
        fn test_mft_entry_pretty_format_with_path() {
            let record_data = create_mock_usn_record(100, 0x12345, 0x67890, "test.txt", 0x20);

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            let formatted =
                entry.pretty_format(Some(std::path::Path::new("C:\\full\\path\\test.txt")));

            assert!(formatted.contains("File ID             : 0x12345"));
            assert!(formatted.contains("Parent File ID      : 0x67890"));
            assert!(formatted.contains("Type                : File"));
            assert!(formatted.contains("Path                : C:\\full\\path\\test.txt"));
        }

        #[test]
        fn test_mft_entry_pretty_format_without_path() {
            let record_data = create_mock_usn_record(
                100, 0x12345, 0x67890, "test.txt", 0x10, // Directory
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(record);

            let formatted = entry.pretty_format(None::<&std::path::Path>);

            assert!(formatted.contains("File ID             : 0x12345"));
            assert!(formatted.contains("Parent File ID      : 0x67890"));
            assert!(formatted.contains("Type                : Directory"));
            assert!(formatted.contains("Path                : test.txt"));
        }
    }

    // Unit tests for EnumOptions
    mod enum_options_tests {}

    // Simplified mocked test using Injectorpp
    mod mocked_tests {
        use super::*;

        #[allow(clippy::too_many_arguments)]
        #[test]
        fn test_device_io_control_error_handling() {
            let mut injector = InjectorPP::new();

            // Mock DeviceIoControl to return an error
            injector
                .when_called(injectorpp::func!(
                    unsafe{} fn (DeviceIoControl)(
                        HANDLE,
                        u32,
                        Option<*const std::ffi::c_void>,
                        u32,
                        Option<*mut std::ffi::c_void>,
                        u32,
                        Option<*mut u32>,
                        Option<*mut windows::Win32::System::IO::OVERLAPPED>
                    ) -> windows::core::Result<()>
                ))
                .will_execute(injectorpp::fake!(
                    func_type: unsafe fn(
                        _handle: HANDLE,
                        _control_code: u32,
                        _input: Option<*const std::ffi::c_void>,
                        _input_size: u32,
                        _output: Option<*mut std::ffi::c_void>,
                        _output_size: u32,
                        _bytes_returned: Option<*mut u32>,
                        _overlapped: Option<*mut windows::Win32::System::IO::OVERLAPPED>
                    ) -> windows::core::Result<()>,
                    returns: Err(windows::core::Error::from(ERROR_INVALID_HANDLE))
                ));

            let volume = Volume {
                handle: HANDLE(std::ptr::null_mut()),
                drive_letter: Some('T'),
                mount_point: None,
            };
            let mft = Mft::new(&volume);

            let mut iter = mft.iter();
            let result = iter.next();

            assert!(result.is_some());
            match result.unwrap() {
                Err(UsnError::WinApiError(_)) => {
                    // Expected error type
                }
                _ => panic!("Expected WinApiError"),
            }
        }
    }
}
