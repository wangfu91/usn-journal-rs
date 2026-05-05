//! Internal helpers for decoding Windows file-attribute bitmasks.

use std::fmt;

/// Shared view over a raw Windows file-attribute bitmask.
pub(crate) trait FileAttributeView {
    /// Returns the underlying Windows file-attribute flags.
    fn file_attributes(&self) -> crate::FileAttributes;

    /// Returns true if the bitmask marks a directory.
    #[inline]
    fn has_directory_attribute(&self) -> bool {
        self.file_attributes()
            .contains(crate::FileAttributes::DIRECTORY)
    }

    /// Returns true if the bitmask marks a hidden item.
    #[inline]
    fn has_hidden_attribute(&self) -> bool {
        self.file_attributes()
            .contains(crate::FileAttributes::HIDDEN)
    }
}

/// Display names for known `FILE_ATTRIBUTE_*` bits.
const FILE_ATTRIBUTE_NAMES: &[(crate::FileAttributes, &str)] = &[
    (crate::FileAttributes::READ_ONLY, "READ_ONLY"),
    (crate::FileAttributes::HIDDEN, "HIDDEN"),
    (crate::FileAttributes::SYSTEM, "SYSTEM"),
    (crate::FileAttributes::DIRECTORY, "DIRECTORY"),
    (crate::FileAttributes::ARCHIVE, "ARCHIVE"),
    (crate::FileAttributes::DEVICE, "DEVICE"),
    (crate::FileAttributes::NORMAL, "NORMAL"),
    (crate::FileAttributes::TEMPORARY, "TEMPORARY"),
    (crate::FileAttributes::SPARSE_FILE, "SPARSE_FILE"),
    (crate::FileAttributes::REPARSE_POINT, "REPARSE_POINT"),
    (crate::FileAttributes::COMPRESSED, "COMPRESSED"),
    (crate::FileAttributes::OFFLINE, "OFFLINE"),
    (
        crate::FileAttributes::NOT_CONTENT_INDEXED,
        "NOT_CONTENT_INDEXED",
    ),
    (crate::FileAttributes::ENCRYPTED, "ENCRYPTED"),
    (crate::FileAttributes::INTEGRITY_STREAM, "INTEGRITY_STREAM"),
    (crate::FileAttributes::VIRTUAL, "VIRTUAL"),
    (crate::FileAttributes::NO_SCRUB_DATA, "NO_SCRUB_DATA"),
    (crate::FileAttributes::RECALL_ON_OPEN, "RECALL_ON_OPEN"),
    (
        crate::FileAttributes::RECALL_ON_DATA_ACCESS,
        "RECALL_ON_DATA_ACCESS",
    ),
];

impl fmt::Display for crate::FileAttributes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote = false;
        for (flag, name) in FILE_ATTRIBUTE_NAMES {
            if self.contains(*flag) {
                if wrote {
                    f.write_str(" | ")?;
                }
                f.write_str(name)?;
                wrote = true;
            }
        }
        if wrote { Ok(()) } else { f.write_str("NONE") }
    }
}
