use std::{ffi::OsString, mem, ptr};

use windows::Win32::Storage::FileSystem::FILE_ID_128;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN};
use windows::Win32::System::Ioctl::{
    USN_JOURNAL_DATA_V0, USN_REASON_CLOSE, USN_REASON_DATA_EXTEND, USN_REASON_FILE_CREATE,
    USN_RECORD_V2, USN_RECORD_V3,
};

use crate::{Fid, Usn};

use super::{UsnEntry, UsnJournalData};

// Mock data generators
fn create_mock_usn_journal_data() -> USN_JOURNAL_DATA_V0 {
    USN_JOURNAL_DATA_V0 {
        UsnJournalID: 0x123456789ABCDEF0,
        FirstUsn: 0x1000,
        NextUsn: 0x5000,
        LowestValidUsn: 0x800,
        MaxUsn: 0x10000,
        MaximumSize: 32 * 1024 * 1024,    // 32MB
        AllocationDelta: 8 * 1024 * 1024, // 8MB
    }
}

fn create_mock_usn_record(
    usn: i64,
    fid: u64,
    parent_fid: u64,
    reason: u32,
    file_name: &str,
    file_attributes: u32,
) -> Vec<u8> {
    let file_name_utf16: Vec<u16> = file_name.encode_utf16().collect();
    let file_name_len = file_name_utf16.len() * mem::size_of::<u16>();
    let base_size = mem::size_of::<USN_RECORD_V2>();
    let total_size = base_size + file_name_len;
    let aligned_size = (total_size + 7) & !7; // 8-byte align

    let mut buffer = vec![0u8; aligned_size];

    // Create USN_RECORD_V2 header - we'll overwrite the FileName area
    let record = USN_RECORD_V2 {
        RecordLength: aligned_size as u32,
        MajorVersion: 2,
        MinorVersion: 0,
        FileReferenceNumber: fid,
        ParentFileReferenceNumber: parent_fid,
        Usn: usn,
        TimeStamp: 0x12345678ABCDEF01i64,
        Reason: reason,
        SourceInfo: 0,
        SecurityId: 0,
        FileAttributes: file_attributes,
        FileNameLength: file_name_len as u16,
        FileNameOffset: mem::offset_of!(USN_RECORD_V2, FileName) as u16,
        FileName: [0; 1],
    };

    // Copy the record header (without the FileName part which we'll handle separately)
    unsafe {
        ptr::copy_nonoverlapping(
            &record as *const USN_RECORD_V2 as *const u8,
            buffer.as_mut_ptr(),
            base_size - mem::size_of::<u16>(), // Exclude the [u16; 1] FileName field
        );
    }

    // Copy the actual filename starting at the FileName offset
    unsafe {
        let filename_ptr = buffer
            .as_mut_ptr()
            .add(mem::offset_of!(USN_RECORD_V2, FileName));
        ptr::copy_nonoverlapping(
            file_name_utf16.as_ptr() as *const u8,
            filename_ptr,
            file_name_len,
        );
    }

    buffer
}

fn create_mock_usn_record_v3(
    usn: i64,
    fid: u128,
    parent_fid: u128,
    reason: u32,
    file_name: &str,
    file_attributes: u32,
) -> Vec<u8> {
    let file_name_utf16: Vec<u16> = file_name.encode_utf16().collect();
    let file_name_len = file_name_utf16.len() * mem::size_of::<u16>();
    let base_size = mem::size_of::<USN_RECORD_V3>();
    let total_size = base_size + file_name_len;
    let aligned_size = (total_size + 7) & !7;

    let mut buffer = vec![0u8; aligned_size];

    let record = USN_RECORD_V3 {
        RecordLength: aligned_size as u32,
        MajorVersion: 3,
        MinorVersion: 0,
        FileReferenceNumber: FILE_ID_128 {
            Identifier: fid.to_le_bytes(),
        },
        ParentFileReferenceNumber: FILE_ID_128 {
            Identifier: parent_fid.to_le_bytes(),
        },
        Usn: usn,
        TimeStamp: 0x12345678ABCDEF01i64,
        Reason: reason,
        SourceInfo: 0,
        SecurityId: 0,
        FileAttributes: file_attributes,
        FileNameLength: file_name_len as u16,
        FileNameOffset: mem::offset_of!(USN_RECORD_V3, FileName) as u16,
        FileName: [0; 1],
    };

    unsafe {
        ptr::copy_nonoverlapping(
            &record as *const USN_RECORD_V3 as *const u8,
            buffer.as_mut_ptr(),
            base_size - mem::size_of::<u16>(),
        );
        let filename_ptr = buffer
            .as_mut_ptr()
            .add(mem::offset_of!(USN_RECORD_V3, FileName));
        ptr::copy_nonoverlapping(
            file_name_utf16.as_ptr() as *const u8,
            filename_ptr,
            file_name_len,
        );
    }

    buffer
}

#[test]
fn test_usn_journal_data_from_conversion() {
    let raw_data = create_mock_usn_journal_data();
    let journal_data = UsnJournalData::from(raw_data);

    assert_eq!(journal_data.journal_id, 0x123456789ABCDEF0);
    assert_eq!(journal_data.first_usn, Usn::new(0x1000));
    assert_eq!(journal_data.next_usn, Usn::new(0x5000));
    assert_eq!(journal_data.lowest_valid_usn, Usn::new(0x800));
    assert_eq!(journal_data.max_usn, Usn::new(0x10000));
    assert_eq!(journal_data.maximum_size, 32 * 1024 * 1024);
    assert_eq!(journal_data.allocation_delta, 8 * 1024 * 1024);
}

#[test]
fn test_usn_entry_creation() {
    let record_data = create_mock_usn_record(
        0x2000,
        0x123456,
        0x654321,
        windows::Win32::System::Ioctl::USN_REASON_FILE_CREATE,
        "test.txt",
        0,
    );

    let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };

    let entry = UsnEntry::new(crate::usn_record::UsnRecordView::V2(record));
    assert_eq!(entry.usn, Usn::new(0x2000));
    assert_eq!(entry.fid, Fid::new(0x123456));
    assert_eq!(entry.parent_fid, Fid::new(0x654321));
    assert_eq!(
        entry.reason,
        windows::Win32::System::Ioctl::USN_REASON_FILE_CREATE
    );
    assert_eq!(entry.file_name, OsString::from("test.txt"));
    assert!(!entry.is_dir());
    assert!(!entry.is_hidden());
}

#[test]
fn test_usn_entry_directory_detection() {
    let record_data = create_mock_usn_record(
        0x3000,
        0x789ABC,
        0x654321,
        windows::Win32::System::Ioctl::USN_REASON_FILE_CREATE,
        "folder",
        FILE_ATTRIBUTE_DIRECTORY.0,
    );

    let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };

    let entry = UsnEntry::new(crate::usn_record::UsnRecordView::V2(record));
    assert!(entry.is_dir());
    assert!(!entry.is_hidden());
}

#[test]
fn test_usn_entry_hidden_detection() {
    let record_data = create_mock_usn_record(
        0x4000,
        0xDEF123,
        0x654321,
        windows::Win32::System::Ioctl::USN_REASON_FILE_CREATE,
        "hidden.txt",
        FILE_ATTRIBUTE_HIDDEN.0,
    );

    let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };

    let entry = UsnEntry::new(crate::usn_record::UsnRecordView::V2(record));
    assert!(!entry.is_dir());
    assert!(entry.is_hidden());
}

#[test]
fn test_usn_entry_reason_string_conversion() {
    let record_data = create_mock_usn_record(
        0x5000,
        0x456789,
        0x654321,
        USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND | USN_REASON_CLOSE,
        "test.txt",
        0,
    );

    let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };

    let entry = UsnEntry::new(crate::usn_record::UsnRecordView::V2(record));
    let reason_string = entry.get_reason_string();

    assert!(reason_string.contains("FILE_CREATE"));
    assert!(reason_string.contains("DATA_EXTEND"));
    assert!(reason_string.contains("CLOSE"));
    assert!(reason_string.contains(" | "));
}

#[test]
fn test_usn_entry_unknown_reason() {
    let record_data = create_mock_usn_record(
        0x6000, 0x789123, 0x654321, 0, // No known reason flags
        "test.txt", 0,
    );

    let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V2) };

    let entry = UsnEntry::new(crate::usn_record::UsnRecordView::V2(record));
    let reason_string = entry.get_reason_string();
    assert_eq!(reason_string, "UNKNOWN");
}

#[test]
fn test_usn_entry_creation_v3_extended_ids() {
    let fid = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeffu128;
    let parent_fid = 0xffee_ddcc_bbaa_9988_7766_5544_3322_1100u128;
    let record_data = create_mock_usn_record_v3(
        0x2000,
        fid,
        parent_fid,
        windows::Win32::System::Ioctl::USN_REASON_FILE_CREATE,
        "refs.txt",
        0,
    );

    let record = unsafe { &*(record_data.as_ptr() as *const USN_RECORD_V3) };
    let entry = UsnEntry::new(crate::usn_record::UsnRecordView::V3(record));

    assert_eq!(entry.usn, Usn::new(0x2000));
    assert_eq!(entry.fid, Fid::from_u128(fid));
    assert_eq!(entry.parent_fid, Fid::from_u128(parent_fid));
    assert_eq!(entry.file_name, OsString::from("refs.txt"));
    assert!(entry.fid.is_extended());
    assert!(entry.parent_fid.is_extended());
}
