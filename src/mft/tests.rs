use super::*;

use injectorpp::interface::injector::*;
use std::mem::offset_of;
use windows::Win32::{
    Foundation::ERROR_INVALID_HANDLE,
    Storage::FileSystem::FILE_ID_128,
    System::Ioctl::{USN_RECORD_V2, USN_RECORD_V3},
};

use crate::{Usn, errors::UsnError, usn_record};

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
        FileName: [0; 1],
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
mod entry {
    use super::*;
    use crate::Fid;

    #[test]
    fn mft_entry_new_basic() {
        let record_data = create_mock_usn_record(
            100,   // usn
            12345, // file_id
            67890, // parent_file_id
            "test.txt", 0x20, // FILE_ATTRIBUTE_ARCHIVE
        );

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert_eq!(entry.usn, Usn::new(100));
        assert_eq!(entry.fid, Fid::new(12345));
        assert_eq!(entry.parent_fid, Fid::new(67890));
        assert_eq!(entry.file_name.to_string_lossy(), "test.txt");
        assert_eq!(entry.file_attributes.bits(), 0x20);
    }

    #[test]
    fn mft_entry_is_dir_true() {
        let record_data = create_mock_usn_record(
            100, 12345, 67890, "folder", 0x10, // FILE_ATTRIBUTE_DIRECTORY
        );

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert!(entry.is_dir());
        assert!(!entry.is_hidden());
    }

    #[test]
    fn mft_entry_is_dir_false() {
        let record_data = create_mock_usn_record(
            100, 12345, 67890, "file.txt", 0x20, // FILE_ATTRIBUTE_ARCHIVE
        );

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert!(!entry.is_dir());
        assert!(!entry.is_hidden());
    }

    #[test]
    fn mft_entry_is_hidden_true() {
        let record_data = create_mock_usn_record(
            100, 12345, 67890, ".hidden", 0x02, // FILE_ATTRIBUTE_HIDDEN
        );

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert!(entry.is_hidden());
        assert!(!entry.is_dir());
    }

    #[test]
    fn mft_entry_combined_attributes() {
        let record_data = create_mock_usn_record(
            100,
            12345,
            67890,
            ".hidden_folder",
            0x12, // FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_HIDDEN
        );

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert!(entry.is_dir());
        assert!(entry.is_hidden());
    }

    #[test]
    fn mft_entry_unicode_filename() {
        let record_data = create_mock_usn_record(100, 12345, 67890, "测试文件.txt", 0x20);

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert_eq!(entry.file_name.to_string_lossy(), "测试文件.txt");
    }

    #[test]
    fn mft_entry_empty_filename() {
        let record_data = create_mock_usn_record(100, 12345, 67890, "", 0x20);

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V2(record));

        assert!(entry.file_name.is_empty());
    }

    #[test]
    fn mft_entry_new_v3_extended_ids() {
        let file_id = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeffu128;
        let parent_id = 0xffee_ddcc_bbaa_9988_7766_5544_3322_1100u128;
        let record_data = create_mock_usn_record_v3(100, file_id, parent_id, "refs.txt", 0x20);

        let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V3) };
        let entry = MftEntry::new(usn_record::UsnRecordView::V3(record));

        assert_eq!(entry.usn, Usn::new(100));
        assert_eq!(entry.fid, Fid::from_u128(file_id));
        assert_eq!(entry.parent_fid, Fid::from_u128(parent_id));
        assert_eq!(entry.file_name.to_string_lossy(), "refs.txt");
        assert_eq!(entry.file_attributes.bits(), 0x20);
    }
}

// Simplified mocked test using Injectorpp
mod mocked {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    #[test]
    fn device_io_control_error_handling() {
        let mut injector = InjectorPP::new();

        // Mock DeviceIoControl to return an error
        crate::test_support::mock_device_io_control!(
            injector,
            Err(windows::core::Error::from(ERROR_INVALID_HANDLE))
        );

        let volume = crate::test_support::mock_volume();
        let mft = Mft::new(&volume);

        let mut iter = mft
            .try_iter()
            .expect("default MFT iterator should be created");
        let result = iter.next();

        assert!(result.is_some());
        match result.expect("mocked iterator should yield one result") {
            Err(UsnError::WinApi(_)) => {}
            _ => panic!("Expected WinApi"),
        }
    }

    #[test]
    fn options_builder_uses_typed_record_version() {
        let options = MftIterOptions::builder()
            .low_usn(Usn::new(10))
            .high_usn(Usn::new(20))
            .max_usn_record_version(UsnRecordVersion::V2)
            .build();

        assert_eq!(options.low_usn, Usn::new(10));
        assert_eq!(options.high_usn, Usn::new(20));
        assert_eq!(options.max_usn_record_version, UsnRecordVersion::V2);
    }
}
