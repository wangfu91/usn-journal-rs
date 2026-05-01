//! Represents the Master File Table (MFT) enumerator for a given NTFS volume.
//!
//! The `Mft` struct provides an iterator interface to enumerate USN records
//! from the MFT using the Windows FSCTL_ENUM_USN_DATA control code. It manages the buffer and state
//! required to sequentially retrieve and parse USN records from the volume.

use crate::{Fid, Usn, UsnResult, errors::UsnError, journal::DEFAULT_BUFFER_BYTES, usn_record, volume::Volume};
use log::debug;
use std::{ffi::OsString, fmt, mem::size_of, os::windows::ffi::OsStringExt};
use windows::Win32::{
    Foundation::{ERROR_HANDLE_EOF, HANDLE},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{self, MFT_ENUM_DATA_V1},
    },
};

/// Represents a single entry returned by `FSCTL_ENUM_USN_DATA`.
///
/// On NTFS the file IDs are standard 64-bit references. On ReFS, when the
/// system returns `USN_RECORD_V3`, `fid` / `parent_fid` hold 128-bit IDs.
#[derive(Debug)]
pub struct MftEntry {
    pub usn: Usn,
    pub fid: Fid,
    pub parent_fid: Fid,
    pub file_name: OsString,
    pub file_attributes: u32,
}

impl MftEntry {
    /// Creates a new `MftEntry` from a validated raw USN record.
    pub(crate) fn new(record: usn_record::UsnRecordRef<'_>) -> Self {
        let file_name_len = record.file_name_length() as usize / std::mem::size_of::<u16>();
        // SAFETY: `record` was returned by `find_next_record`, which has
        // validated that `FileName` plus `FileNameLength` lies entirely
        // within the record's buffer. The MFT FSCTL output uses the same
        // trailing UTF-16 layout for `USN_RECORD_V2` and `USN_RECORD_V3`,
        // so this is identical to `UsnEntry::new`.
        let file_name_data =
            unsafe { std::slice::from_raw_parts(record.file_name_ptr(), file_name_len) };
        let file_name = OsString::from_wide(file_name_data);

        MftEntry {
            usn: Usn::new(record.usn()),
            fid: record.fid(),
            parent_fid: record.parent_fid(),
            file_name,
            file_attributes: record.file_attributes(),
        }
    }

    /// Returns true if this entry represents a directory.
    #[must_use]
    #[inline]
    pub fn is_dir(&self) -> bool {
        let attributes = FILE_FLAGS_AND_ATTRIBUTES(self.file_attributes);
        attributes.contains(FILE_ATTRIBUTE_DIRECTORY)
    }

    /// Returns true if this entry represents a hidden file or directory.
    #[must_use]
    #[inline]
    pub fn is_hidden(&self) -> bool {
        let attributes = FILE_FLAGS_AND_ATTRIBUTES(self.file_attributes);
        attributes.contains(FILE_ATTRIBUTE_HIDDEN)
    }

    /// Strongly-typed view of [`MftEntry::file_attributes`].
    ///
    /// Unknown bits are preserved.
    #[must_use]
    #[inline]
    pub fn file_attributes_flags(&self) -> crate::FileAttributes {
        crate::FileAttributes::from_bits_retain(self.file_attributes)
    }
}

impl fmt::Display for MftEntry {
    /// One-line, compact summary suitable for logging. For a multi-line
    /// "pretty" rendering see `examples/pretty_print.rs`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MFT fid={} parent={} attrs=0x{:x} \"{}\"",
            self.fid,
            self.parent_fid,
            self.file_attributes,
            self.file_name.to_string_lossy(),
        )
    }
}

/// Options for enumerating the Master File Table (MFT).
///
/// Allows customization of the USN range and buffer size for enumeration.
///
/// Use [`MftIterOptions::builder`] for the fluent builder API, or construct
/// directly via struct-literal syntax. [`Default`] is also implemented.
#[derive(Debug, Clone)]
pub struct MftIterOptions {
    pub low_usn: Usn,
    pub high_usn: Usn,
    pub buffer_size: usize,
}

impl Default for MftIterOptions {
    fn default() -> Self {
        MftIterOptions {
            low_usn: Usn::new(0),
            high_usn: Usn::new(i64::MAX),
            buffer_size: DEFAULT_BUFFER_BYTES,
        }
    }
}

impl MftIterOptions {
    /// Returns a fluent builder for [`MftIterOptions`].
    pub fn builder() -> MftIterOptionsBuilder {
        MftIterOptionsBuilder::default()
    }
}

/// Fluent builder for [`MftIterOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct MftIterOptionsBuilder {
    inner: MftIterOptions,
}

impl MftIterOptionsBuilder {
    /// Set the inclusive lower USN bound.
    pub fn low_usn(mut self, v: Usn) -> Self {
        self.inner.low_usn = v;
        self
    }

    /// Set the inclusive upper USN bound.
    pub fn high_usn(mut self, v: Usn) -> Self {
        self.inner.high_usn = v;
        self
    }

    /// Set the in-memory buffer size, in bytes.
    pub fn buffer_size(mut self, v: usize) -> Self {
        self.inner.buffer_size = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> MftIterOptions {
        self.inner
    }
}

/// Represents the Master File Table (MFT) enumerator.
#[derive(Debug)]
pub struct Mft<'a> {
    pub(crate) volume: &'a Volume,
}

impl<'a> Mft<'a> {
    /// Creates a new `Mft` instance.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        Mft { volume }
    }

    /// Returns an iterator over the MFT entries.
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    pub fn try_iter(&self) -> UsnResult<MftIter> {
        self.try_iter_with_options(MftIterOptions::default())
    }

    /// Returns an iterator over the MFT entries with custom options.
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    pub fn try_iter_with_options(&self, options: MftIterOptions) -> UsnResult<MftIter> {
        if options.buffer_size == 0 {
            return Err(UsnError::InvalidOptions(
                "buffer_size must be greater than 0",
            ));
        }

        Ok(MftIter {
            volume_handle: self.volume.handle,
            low_usn: options.low_usn.get(),
            high_usn: options.high_usn.get(),
            buffer: vec![0u8; options.buffer_size],
            bytes_read: 0,
            offset: 0,
            next_start_fid: options.low_usn.get() as u64,
        })
    }
}

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
    /// Swap in a caller-provided buffer to avoid allocating during long
    /// iteration loops. The buffer is cleared and resized to the
    /// originally requested capacity (the configured `MftIterOptions::buffer_size`).
    /// This is purely additive and may be invoked any number of times
    /// before the first `next()` call.
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
    fn find_next_entry(&mut self) -> Result<Option<usn_record::UsnRecordRef<'_>>, UsnError> {
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
mod tests {
    use super::*;
    use injectorpp::interface::injector::*;
    use std::mem::offset_of;
    use windows::Win32::{
        Foundation::{ERROR_INVALID_HANDLE, HANDLE},
        Storage::FileSystem::FILE_ID_128,
        System::Ioctl::{USN_RECORD_V2, USN_RECORD_V3},
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

    #[allow(clippy::too_many_arguments)]
    fn create_mock_usn_record_v3(
        usn: i64,
        file_id: u128,
        parent_file_id: u128,
        file_name: &str,
        file_attributes: u32,
    ) -> Vec<u8> {
        let file_name_utf16: Vec<u16> = file_name.encode_utf16().collect();
        let file_name_len = file_name_utf16.len() * 2;
        let filename_offset = offset_of!(USN_RECORD_V3, FileName);
        let record_len = filename_offset + file_name_len;
        let mut buffer = vec![0u8; record_len];

        let base_record = USN_RECORD_V3 {
            RecordLength: record_len as u32,
            MajorVersion: 3,
            MinorVersion: 0,
            FileReferenceNumber: FILE_ID_128 {
                Identifier: file_id.to_le_bytes(),
            },
            ParentFileReferenceNumber: FILE_ID_128 {
                Identifier: parent_file_id.to_le_bytes(),
            },
            Usn: usn,
            TimeStamp: 0,
            Reason: 0,
            SourceInfo: 0,
            SecurityId: 0,
            FileAttributes: file_attributes,
            FileNameLength: file_name_len as u16,
            FileNameOffset: filename_offset as u16,
            FileName: [0; 1],
        };

        unsafe {
            std::ptr::copy_nonoverlapping(
                &base_record as *const USN_RECORD_V3 as *const u8,
                buffer.as_mut_ptr(),
                filename_offset,
            );
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
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

            assert_eq!(entry.usn, Usn::new(100));
            assert_eq!(entry.fid, Fid::new(12345));
            assert_eq!(entry.parent_fid, Fid::new(67890));
            assert_eq!(entry.file_name.to_string_lossy(), "test.txt");
            assert_eq!(entry.file_attributes, 0x20);
        }

        #[test]
        fn test_mft_entry_is_dir_true() {
            let record_data = create_mock_usn_record(
                100, 12345, 67890, "folder", 0x10, // FILE_ATTRIBUTE_DIRECTORY
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

            assert!(entry.is_dir());
            assert!(!entry.is_hidden());
        }

        #[test]
        fn test_mft_entry_is_dir_false() {
            let record_data = create_mock_usn_record(
                100, 12345, 67890, "file.txt", 0x20, // FILE_ATTRIBUTE_ARCHIVE
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

            assert!(!entry.is_dir());
            assert!(!entry.is_hidden());
        }

        #[test]
        fn test_mft_entry_is_hidden_true() {
            let record_data = create_mock_usn_record(
                100, 12345, 67890, ".hidden", 0x02, // FILE_ATTRIBUTE_HIDDEN
            );

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

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
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

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
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

            assert_eq!(entry.file_name.to_string_lossy(), "测试文件.txt");
        }

        #[test]
        fn test_mft_entry_empty_filename() {
            let record_data = create_mock_usn_record(100, 12345, 67890, "", 0x20);

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

            assert!(entry.file_name.is_empty());
        }

        #[test]
        fn test_mft_entry_display_smoke() {
            let record_data = create_mock_usn_record(100, 0x12345, 0x67890, "test.txt", 0x20);

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
            let entry = MftEntry::new(usn_record::UsnRecordRef::V2(record));

            let formatted = format!("{entry}");

            assert!(formatted.contains("0x12345"));
            assert!(formatted.contains("0x67890"));
            assert!(formatted.contains("test.txt"));
        }

        #[test]
        fn test_mft_entry_new_v3_extended_ids() {
            let file_id = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeffu128;
            let parent_id = 0xffee_ddcc_bbaa_9988_7766_5544_3322_1100u128;
            let record_data =
                create_mock_usn_record_v3(100, file_id, parent_id, "refs.txt", 0x20);

            let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V3) };
            let entry = MftEntry::new(usn_record::UsnRecordRef::V3(record));

            assert_eq!(entry.usn, Usn::new(100));
            assert_eq!(entry.fid, Fid::from_u128(file_id));
            assert_eq!(entry.parent_fid, Fid::from_u128(parent_id));
            assert_eq!(entry.file_name.to_string_lossy(), "refs.txt");
            assert_eq!(entry.file_attributes, 0x20);
        }
    }

    // Unit tests for MftIterOptions
    mod enum_options_tests {}

    // Simplified mocked test using Injectorpp
    mod mocked_tests {
        use super::*;

        #[allow(clippy::too_many_arguments)]
        #[test]
        fn test_device_io_control_error_handling() {
            let mut injector = InjectorPP::new();

            // Mock DeviceIoControl to return an error
            crate::test_support::mock_device_io_control!(
                injector,
                Err(windows::core::Error::from(ERROR_INVALID_HANDLE))
            );

            let volume = crate::test_support::mock_volume();
            let mft = Mft::new(&volume);

            let mut iter = mft.try_iter().expect("default MFT iterator should be created");
            let result = iter.next();

            assert!(result.is_some());
            match result.unwrap() {
                Err(UsnError::WinApi(_)) => {
                    // Expected error type
                }
                _ => panic!("Expected WinApi"),
            }
        }

        #[test]
        fn test_iter_with_invalid_buffer_size() {
            let volume = Volume::mock(HANDLE(std::ptr::null_mut()), crate::volume::VolumeSource::DriveLetter('T'));
            let mft = Mft::new(&volume);

            let result = mft.try_iter_with_options(MftIterOptions {
                low_usn: Usn::new(0),
                high_usn: Usn::new(i64::MAX),
                buffer_size: 0,
            });

            assert!(matches!(
                result,
                Err(UsnError::InvalidOptions(
                    "buffer_size must be greater than 0"
                ))
            ));
        }
    }
}
