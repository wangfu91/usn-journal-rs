//! Internal helpers for decoding Windows file-attribute bitmasks.

use std::fmt;

/// Shared view over a raw Windows file-attribute bitmask.
pub(crate) trait FileAttributeView {
    /// Returns the underlying Windows file-attribute flags.
    fn file_attributes(&self) -> crate::FileAttributes;

    /// Returns true if the bitmask marks a directory.
    #[inline]
    fn has_directory_attribute(&self) -> bool {
        self.file_attributes().is_directory()
    }

    /// Returns true if the bitmask marks a hidden item.
    #[inline]
    fn has_hidden_attribute(&self) -> bool {
        self.file_attributes().is_hidden()
    }
}

/// Display names for known `FILE_ATTRIBUTE_*` bits.
const FILE_ATTRIBUTE_NAMES: &[(u32, &str)] = &[
    (crate::FileAttributes::READ_ONLY.bits(), "READ_ONLY"),
    (crate::FileAttributes::HIDDEN.bits(), "HIDDEN"),
    (crate::FileAttributes::SYSTEM.bits(), "SYSTEM"),
    (crate::FileAttributes::DIRECTORY.bits(), "DIRECTORY"),
    (crate::FileAttributes::ARCHIVE.bits(), "ARCHIVE"),
    (crate::FileAttributes::DEVICE.bits(), "DEVICE"),
    (crate::FileAttributes::NORMAL.bits(), "NORMAL"),
    (crate::FileAttributes::TEMPORARY.bits(), "TEMPORARY"),
    (crate::FileAttributes::SPARSE_FILE.bits(), "SPARSE_FILE"),
    (crate::FileAttributes::REPARSE_POINT.bits(), "REPARSE_POINT"),
    (crate::FileAttributes::COMPRESSED.bits(), "COMPRESSED"),
    (crate::FileAttributes::OFFLINE.bits(), "OFFLINE"),
    (
        crate::FileAttributes::NOT_CONTENT_INDEXED.bits(),
        "NOT_CONTENT_INDEXED",
    ),
    (crate::FileAttributes::ENCRYPTED.bits(), "ENCRYPTED"),
    (
        crate::FileAttributes::INTEGRITY_STREAM.bits(),
        "INTEGRITY_STREAM",
    ),
    (crate::FileAttributes::VIRTUAL.bits(), "VIRTUAL"),
    (crate::FileAttributes::NO_SCRUB_DATA.bits(), "NO_SCRUB_DATA"),
    (crate::FileAttributes::RECALL_ON_OPEN.bits(), "RECALL_ON_OPEN"),
    (
        crate::FileAttributes::RECALL_ON_DATA_ACCESS.bits(),
        "RECALL_ON_DATA_ACCESS",
    ),
];

impl fmt::Display for crate::FileAttributes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::display::write_flag_names(f, self.bits(), FILE_ATTRIBUTE_NAMES, " | ")
    }
}
