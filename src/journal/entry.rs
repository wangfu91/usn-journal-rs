//! USN journal entry representation.

use std::fmt;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};

use crate::file_attributes::FileAttributeView;
use crate::usn_record::UsnRecordView;
use crate::{Fid, Filetime, Usn};

use super::reason::format_reason;

/// Owned representation of a USN journal entry.
///
/// `fid` / `parent_fid` may be either standard 64-bit NTFS file references
/// or 128-bit file IDs from `USN_RECORD_V3` on ReFS.
#[derive(Debug)]
pub struct UsnEntry {
    /// Parsed Update Sequence Number.
    pub usn: Usn,
    /// Parsed FILETIME timestamp.
    pub time: Filetime,
    /// Parsed file identifier.
    pub fid: Fid,
    /// Parsed parent file identifier.
    pub parent_fid: Fid,
    /// Raw USN reason bitmask.
    pub reason: u32,
    /// Raw source-info bitmask.
    pub source_info: u32,
    /// Parsed file name.
    pub file_name: OsString,
    /// Raw file-attribute bitmask.
    pub file_attributes: u32,
}

impl UsnEntry {
    /// Create a new `UsnEntry` from a validated raw USN record view.
    ///
    /// # Arguments
    /// * `record` - Borrowed `USN_RECORD_V2` or `USN_RECORD_V3` view.
    ///
    /// # Returns
    /// A parsed `UsnEntry` with decoded fields and file name.
    pub(crate) fn new(record: UsnRecordView<'_>) -> Self {
        let file_name = OsString::from_wide(record.file_name_slice());

        UsnEntry {
            usn: Usn::new(record.usn()),
            time: Filetime::new(if record.timestamp() < 0 {
                0
            } else {
                record.timestamp() as u64
            }),
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
        <Self as FileAttributeView>::has_directory_attribute(self)
    }

    /// Returns true if this entry represents a hidden file or directory.
    #[must_use]
    #[inline]
    pub fn is_hidden(&self) -> bool {
        <Self as FileAttributeView>::has_hidden_attribute(self)
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
        <Self as FileAttributeView>::file_attribute_flags(self)
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

impl FileAttributeView for UsnEntry {
    fn raw_file_attributes(&self) -> u32 {
        self.file_attributes
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
