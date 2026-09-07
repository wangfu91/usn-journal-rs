//! USN journal entry representation.

use std::fmt;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};

use crate::usn_record::UsnRecordView;
use crate::{Fid, FileAttributes, Filetime, Usn, UsnReason, UsnSourceInfo};

use super::reason::CompactReason;

/// Owned representation of a USN journal entry.
///
/// `fid` / `parent_fid` may be either standard 64-bit NTFS file references
/// or 128-bit file IDs from `USN_RECORD_V3` on ReFS.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsnEntry {
    /// Parsed Update Sequence Number.
    pub usn: Usn,
    /// Parsed FILETIME timestamp.
    pub time: Filetime,
    /// Parsed file identifier.
    pub fid: Fid,
    /// Parsed parent file identifier.
    pub parent_fid: Fid,
    /// USN reason flags.
    pub reason: UsnReason,
    /// Source-info flags.
    pub source_info: UsnSourceInfo,
    /// Parsed file name.
    pub file_name: OsString,
    /// File-attribute flags.
    pub file_attributes: FileAttributes,
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
        let file_name = OsString::from_wide(&record.file_name_slice());

        UsnEntry {
            usn: Usn::new(record.usn()),
            time: Filetime::new(if record.timestamp() < 0 {
                0
            } else {
                record.timestamp() as u64
            }),
            fid: record.fid(),
            parent_fid: record.parent_fid(),
            reason: UsnReason::from_bits_retain(record.reason()),
            source_info: UsnSourceInfo::from_bits_retain(record.source_info()),
            file_name,
            file_attributes: FileAttributes::from_bits_retain(record.file_attributes()),
        }
    }

    /// Returns true if this entry represents a directory.
    #[must_use]
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.file_attributes.is_directory()
    }

    /// Returns true if this entry represents a hidden file or directory.
    #[must_use]
    #[inline]
    pub fn is_hidden(&self) -> bool {
        self.file_attributes.is_hidden()
    }

    /// Returns true if this entry is marked read-only.
    #[must_use]
    #[inline]
    pub fn is_read_only(&self) -> bool {
        self.file_attributes.is_read_only()
    }

    /// Returns true if this entry has the system attribute set.
    #[must_use]
    #[inline]
    pub fn is_system(&self) -> bool {
        self.file_attributes.is_system()
    }

    /// Returns true if this entry has the archive attribute set.
    #[must_use]
    #[inline]
    pub fn is_archive(&self) -> bool {
        self.file_attributes.is_archive()
    }

    /// Returns true if this entry is a reparse point (symlink, junction, mount point, ...).
    #[must_use]
    #[inline]
    pub fn is_reparse_point(&self) -> bool {
        self.file_attributes.is_reparse_point()
    }

    /// Returns true if this entry is stored compressed on disk.
    #[must_use]
    #[inline]
    pub fn is_compressed(&self) -> bool {
        self.file_attributes.is_compressed()
    }

    /// Returns true if this entry is stored encrypted on disk.
    #[must_use]
    #[inline]
    pub fn is_encrypted(&self) -> bool {
        self.file_attributes.is_encrypted()
    }

    /// Returns true if this entry contains sparse data.
    #[must_use]
    #[inline]
    pub fn is_sparse(&self) -> bool {
        self.file_attributes.is_sparse()
    }
}

impl fmt::Display for UsnEntry {
    /// One-line, compact summary suitable for logging. For a multi-line
    /// "pretty" rendering see `examples/journal_pretty_print.rs`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "USN 0x{:x} [{}] fid={} parent={} attrs=0x{:x} \"{}\"",
            self.usn.get(),
            CompactReason(self.reason),
            self.fid,
            self.parent_fid,
            self.file_attributes.bits(),
            self.file_name.to_string_lossy(),
        )
    }
}
