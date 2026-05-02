//! Owned representation of records returned by `FSCTL_ENUM_USN_DATA`.

use std::fmt;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};

use crate::file_attributes::FileAttributeView;
use crate::usn_record::UsnRecordView;
use crate::{Fid, FileAttributes, Usn};

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
    /// File-attribute flags.
    pub file_attributes: FileAttributes,
}

impl MftEntry {
    /// Create a new `MftEntry` from a validated raw USN record view.
    pub(crate) fn new(record: UsnRecordView<'_>) -> Self {
        let file_name = OsString::from_wide(record.file_name_slice());

        MftEntry {
            usn: Usn::new(record.usn()),
            fid: record.fid(),
            parent_fid: record.parent_fid(),
            file_name,
            file_attributes: FileAttributes::from_bits_retain(record.file_attributes()),
        }
    }

    /// Returns true if this entry represents a directory.
    #[must_use]
    #[inline]
    pub fn is_dir(&self) -> bool {
        <Self as FileAttributeView>::has_directory_attribute(self)
    }

    /// Returns true if this entry represents a hidden file or directory.
    #[must_use]
    #[inline]
    pub fn is_hidden(&self) -> bool {
        <Self as FileAttributeView>::has_hidden_attribute(self)
    }

    /// Strongly-typed view of [`MftEntry::file_attributes`].
    ///
    /// Unknown bits are preserved.
    #[must_use]
    #[inline]
    pub fn file_attributes_flags(&self) -> crate::FileAttributes {
        <Self as FileAttributeView>::file_attribute_flags(self)
    }

    /// Raw file-attribute bitmask.
    #[must_use]
    #[inline]
    pub fn raw_file_attributes(&self) -> u32 {
        self.file_attributes.bits()
    }
}

impl FileAttributeView for MftEntry {
    fn file_attributes(&self) -> FileAttributes {
        self.file_attributes
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
            self.file_attributes.bits(),
            self.file_name.to_string_lossy(),
        )
    }
}
