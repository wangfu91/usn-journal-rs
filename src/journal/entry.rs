//! USN journal entry representation.

use std::fmt;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
};

use crate::time::Filetime;
use crate::usn_record::UsnRecordRef;
use crate::{Fid, Usn};

use super::reason::format_reason;

/// Represents a USN entry in the USN journal.
///
/// `fid` / `parent_fid` may be either standard 64-bit NTFS file references
/// or 128-bit file IDs from `USN_RECORD_V3` on ReFS.
#[derive(Debug)]
pub struct UsnEntry {
    pub usn: Usn,
    pub time: Filetime,
    pub fid: Fid,
    pub parent_fid: Fid,
    pub reason: u32,
    pub source_info: u32,
    pub file_name: OsString,
    pub file_attributes: u32,
}

impl UsnEntry {
    /// Create a new `UsnEntry` from a validated raw USN record.
    ///
    /// # Arguments
    /// * `record` - Reference to a validated `USN_RECORD_V2` or `USN_RECORD_V3`
    ///   structure from the Windows API.
    ///
    /// # Returns
    /// A parsed `UsnEntry` with decoded fields and file name.
    pub(crate) fn new(record: UsnRecordRef<'_>) -> Self {
        let file_name_len = record.file_name_length() as usize / std::mem::size_of::<u16>();

        // https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2
        // When working with FileName, do not count on the file name that contains a trailing '\0' delimiter,
        // but instead determine the length of the file name by using FileNameLength.
        // Do not perform any compile-time pointer arithmetic using FileName.
        // Instead, make necessary calculations at run time by using the value of the FileNameOffset member.
        // Doing so helps make your code compatible with any future versions of the USN record.
        // SAFETY: `record` is a validated `USN_RECORD_V2` / `USN_RECORD_V3` reference that
        // came from `find_next_record`, which has already checked that
        // `FileName` plus `FileNameLength` lies entirely within the
        // record's `RecordLength`. `FileName` is laid out in memory as
        // `FileNameLength` bytes (== `file_name_len` u16 code units)
        // starting at `record.file_name_ptr()`.
        let file_name_data =
            unsafe { std::slice::from_raw_parts(record.file_name_ptr(), file_name_len) };
        let file_name = OsString::from_wide(file_name_data);

        UsnEntry {
            usn: Usn::new(record.usn()),
            time: Filetime::from_raw_i64(record.timestamp()),
            fid: record.fid(),
            parent_fid: record.parent_fid(),
            reason: record.reason(),
            source_info: record.source_info(),
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

    /// Strongly-typed view of [`UsnEntry::reason`].
    ///
    /// Unknown bits are preserved.
    #[must_use]
    #[inline]
    pub fn reason_flags(&self) -> crate::UsnReason {
        crate::UsnReason::from_bits_retain(self.reason)
    }

    /// Strongly-typed view of [`UsnEntry::file_attributes`].
    ///
    /// Unknown bits are preserved.
    #[must_use]
    #[inline]
    pub fn file_attributes_flags(&self) -> crate::FileAttributes {
        crate::FileAttributes::from_bits_retain(self.file_attributes)
    }

    /// Converts a USN reason bitfield to a human-readable string using Windows constants.
    #[must_use]
    pub fn get_reason_string(&self) -> String {
        format_reason(self.reason)
    }

    /// Formats a compact reason summary using `|` as separator (no spaces).
    fn reason_compact(&self) -> String {
        self.get_reason_string().replace(" | ", "|")
    }
}

impl fmt::Display for UsnEntry {
    /// One-line, compact summary suitable for logging. For a multi-line
    /// "pretty" rendering see `examples/pretty_print.rs`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "USN 0x{:x} [{}] fid={} parent={} attrs=0x{:x} \"{}\"",
            self.usn.get(),
            self.reason_compact(),
            self.fid,
            self.parent_fid,
            self.file_attributes,
            self.file_name.to_string_lossy(),
        )
    }
}
