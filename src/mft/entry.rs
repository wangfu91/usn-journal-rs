//! Owned representation of records returned by `FSCTL_ENUM_USN_DATA`.

use std::fmt;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
};

use crate::usn_record::UsnRecordView;
use crate::{Fid, Usn};

/// Owned representation of a single entry returned by `FSCTL_ENUM_USN_DATA`.
///
/// On NTFS the file IDs are standard 64-bit references. On ReFS, when the
/// system returns `USN_RECORD_V3`, `fid` / `parent_fid` hold 128-bit IDs.
#[derive(Debug)]
pub struct MftEntry {
    /// Parsed Update Sequence Number.
    pub usn: Usn,
    /// Parsed file identifier.
    pub fid: Fid,
    /// Parsed parent file identifier.
    pub parent_fid: Fid,
    /// Parsed file name.
    pub file_name: OsString,
    /// Raw file-attribute bitmask.
    pub file_attributes: u32,
}

impl MftEntry {
    /// Create a new `MftEntry` from a validated raw USN record view.
    pub(crate) fn new(record: UsnRecordView<'_>) -> Self {
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