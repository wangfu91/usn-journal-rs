use std::{ffi::OsString, os::windows::ffi::OsStringExt, time::SystemTime};

use windows::Win32::System::Ioctl::{
    USN_REASON_BASIC_INFO_CHANGE, USN_REASON_CLOSE, USN_REASON_COMPRESSION_CHANGE,
    USN_REASON_DATA_EXTEND, USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION,
    USN_REASON_DESIRED_STORAGE_CLASS_CHANGE, USN_REASON_EA_CHANGE, USN_REASON_ENCRYPTION_CHANGE,
    USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE,
    USN_REASON_INDEXABLE_CHANGE, USN_REASON_INTEGRITY_CHANGE, USN_REASON_NAMED_DATA_EXTEND,
    USN_REASON_NAMED_DATA_OVERWRITE, USN_REASON_NAMED_DATA_TRUNCATION, USN_REASON_OBJECT_ID_CHANGE,
    USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME, USN_REASON_REPARSE_POINT_CHANGE,
    USN_REASON_SECURITY_CHANGE, USN_REASON_STREAM_CHANGE, USN_REASON_TRANSACTED_CHANGE,
};
use windows::Win32::{
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
    },
    System::Ioctl::USN_RECORD_V2,
};

use crate::{Usn, utils};

/// Represents a USN entry in the USN journal.
#[derive(Debug)]
pub struct UsnEntry {
    pub usn: Usn,
    pub time: SystemTime,
    pub fid: u64,
    pub parent_fid: u64,
    pub reason: u32,
    pub source_info: u32,
    pub file_name: OsString,
    pub file_attributes: FILE_FLAGS_AND_ATTRIBUTES,
}

impl UsnEntry {
    /// Create a new `UsnEntry` from a raw USN_RECORD_V2 record.
    ///
    /// # Arguments
    /// * `record` - Reference to a USN_RECORD_V2 structure from the Windows API.
    ///
    /// # Returns
    /// A parsed `UsnEntry` with decoded fields and file name.
    pub(crate) fn new(record: &USN_RECORD_V2) -> Self {
        let file_name_len = record.FileNameLength as usize / std::mem::size_of::<u16>();

        // https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2
        // When working with FileName, do not count on the file name that contains a trailing '\0' delimiter,
        // but instead determine the length of the file name by using FileNameLength.
        // Do not perform any compile-time pointer arithmetic using FileName.
        // Instead, make necessary calculations at run time by using the value of the FileNameOffset member.
        // Doing so helps make your code compatible with any future versions of USN_RECORD_V2.
        let file_name_data =
            unsafe { std::slice::from_raw_parts(record.FileName.as_ptr(), file_name_len) };
        let file_name = OsString::from_wide(file_name_data);

        let sys_time =
            utils::filetime_to_systemtime(record.TimeStamp).unwrap_or(SystemTime::UNIX_EPOCH);
        UsnEntry {
            usn: record.Usn,
            time: sys_time,
            fid: record.FileReferenceNumber,
            parent_fid: record.ParentFileReferenceNumber,
            reason: record.Reason,
            source_info: record.SourceInfo,
            file_name,
            file_attributes: FILE_FLAGS_AND_ATTRIBUTES(record.FileAttributes),
        }
    }

    /// Returns true if this entry represents a directory.
    pub fn is_dir(&self) -> bool {
        self.file_attributes.contains(FILE_ATTRIBUTE_DIRECTORY)
    }

    /// Returns true if this entry represents a hidden file or directory.
    pub fn is_hidden(&self) -> bool {
        self.file_attributes.contains(FILE_ATTRIBUTE_HIDDEN)
    }

    /// Converts a USN reason bitfield to a human-readable string using Windows constants.
    pub fn reason_to_string(&self) -> String {
        let reason = self.reason;
        let mut reasons = Vec::new();
        if reason & USN_REASON_DATA_OVERWRITE != 0 {
            reasons.push("DATA_OVERWRITE");
        }
        if reason & USN_REASON_DATA_EXTEND != 0 {
            reasons.push("DATA_EXTEND");
        }
        if reason & USN_REASON_DATA_TRUNCATION != 0 {
            reasons.push("DATA_TRUNCATION");
        }
        if reason & USN_REASON_NAMED_DATA_OVERWRITE != 0 {
            reasons.push("NAMED_DATA_OVERWRITE");
        }
        if reason & USN_REASON_NAMED_DATA_EXTEND != 0 {
            reasons.push("NAMED_DATA_EXTEND");
        }
        if reason & USN_REASON_NAMED_DATA_TRUNCATION != 0 {
            reasons.push("NAMED_DATA_TRUNCATION");
        }
        if reason & USN_REASON_FILE_CREATE != 0 {
            reasons.push("FILE_CREATE");
        }
        if reason & USN_REASON_FILE_DELETE != 0 {
            reasons.push("FILE_DELETE");
        }
        if reason & USN_REASON_EA_CHANGE != 0 {
            reasons.push("EA_CHANGE");
        }
        if reason & USN_REASON_SECURITY_CHANGE != 0 {
            reasons.push("SECURITY_CHANGE");
        }
        if reason & USN_REASON_RENAME_OLD_NAME != 0 {
            reasons.push("RENAME_OLD_NAME");
        }
        if reason & USN_REASON_RENAME_NEW_NAME != 0 {
            reasons.push("RENAME_NEW_NAME");
        }
        if reason & USN_REASON_INDEXABLE_CHANGE != 0 {
            reasons.push("INDEXABLE_CHANGE");
        }
        if reason & USN_REASON_BASIC_INFO_CHANGE != 0 {
            reasons.push("BASIC_INFO_CHANGE");
        }
        if reason & USN_REASON_HARD_LINK_CHANGE != 0 {
            reasons.push("HARD_LINK_CHANGE");
        }
        if reason & USN_REASON_COMPRESSION_CHANGE != 0 {
            reasons.push("COMPRESSION_CHANGE");
        }
        if reason & USN_REASON_ENCRYPTION_CHANGE != 0 {
            reasons.push("ENCRYPTION_CHANGE");
        }
        if reason & USN_REASON_OBJECT_ID_CHANGE != 0 {
            reasons.push("OBJECT_ID_CHANGE");
        }
        if reason & USN_REASON_REPARSE_POINT_CHANGE != 0 {
            reasons.push("REPARSE_POINT_CHANGE");
        }
        if reason & USN_REASON_STREAM_CHANGE != 0 {
            reasons.push("STREAM_CHANGE");
        }
        if reason & USN_REASON_TRANSACTED_CHANGE != 0 {
            reasons.push("TRANSACTED_CHANGE");
        }
        if reason & USN_REASON_INTEGRITY_CHANGE != 0 {
            reasons.push("INTEGRITY_CHANGE");
        }
        if reason & USN_REASON_DESIRED_STORAGE_CLASS_CHANGE != 0 {
            reasons.push("DESIRED_STORAGE_CLASS_CHANGE");
        }
        if reason & USN_REASON_CLOSE != 0 {
            reasons.push("CLOSE");
        }
        if reasons.is_empty() {
            reasons.push("UNKNOWN");
        }
        reasons.join(" | ")
    }
}
