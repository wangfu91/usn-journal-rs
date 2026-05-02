//! Helpers for formatting USN reason bitfields.

use std::fmt;

use crate::UsnReason;

const REASON_FLAGS: &[(UsnReason, &str)] = &[
    (UsnReason::DATA_OVERWRITE, "DATA_OVERWRITE"),
    (UsnReason::DATA_EXTEND, "DATA_EXTEND"),
    (UsnReason::DATA_TRUNCATION, "DATA_TRUNCATION"),
    (UsnReason::NAMED_DATA_OVERWRITE, "NAMED_DATA_OVERWRITE"),
    (UsnReason::NAMED_DATA_EXTEND, "NAMED_DATA_EXTEND"),
    (UsnReason::NAMED_DATA_TRUNCATION, "NAMED_DATA_TRUNCATION"),
    (UsnReason::FILE_CREATE, "FILE_CREATE"),
    (UsnReason::FILE_DELETE, "FILE_DELETE"),
    (UsnReason::EA_CHANGE, "EA_CHANGE"),
    (UsnReason::SECURITY_CHANGE, "SECURITY_CHANGE"),
    (UsnReason::RENAME_OLD_NAME, "RENAME_OLD_NAME"),
    (UsnReason::RENAME_NEW_NAME, "RENAME_NEW_NAME"),
    (UsnReason::INDEXABLE_CHANGE, "INDEXABLE_CHANGE"),
    (UsnReason::BASIC_INFO_CHANGE, "BASIC_INFO_CHANGE"),
    (UsnReason::HARD_LINK_CHANGE, "HARD_LINK_CHANGE"),
    (UsnReason::COMPRESSION_CHANGE, "COMPRESSION_CHANGE"),
    (UsnReason::ENCRYPTION_CHANGE, "ENCRYPTION_CHANGE"),
    (UsnReason::OBJECT_ID_CHANGE, "OBJECT_ID_CHANGE"),
    (UsnReason::REPARSE_POINT_CHANGE, "REPARSE_POINT_CHANGE"),
    (UsnReason::STREAM_CHANGE, "STREAM_CHANGE"),
    (UsnReason::TRANSACTED_CHANGE, "TRANSACTED_CHANGE"),
    (UsnReason::INTEGRITY_CHANGE, "INTEGRITY_CHANGE"),
    (
        UsnReason::DESIRED_STORAGE_CLASS_CHANGE,
        "DESIRED_STORAGE_CLASS_CHANGE",
    ),
    (UsnReason::CLOSE, "CLOSE"),
];

/// Convert a USN reason bitfield to a human-readable string.
pub(super) fn format_reason(reason: UsnReason) -> String {
    reason.to_string()
}

pub(super) struct CompactReason(pub(super) UsnReason);

impl fmt::Display for CompactReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_reason(self.0, f, "|")
    }
}

impl fmt::Display for UsnReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_reason(*self, f, " | ")
    }
}

fn fmt_reason(reason: UsnReason, f: &mut fmt::Formatter<'_>, separator: &str) -> fmt::Result {
    let mut wrote = false;
    for (flag, name) in REASON_FLAGS {
        if reason.contains(*flag) {
            if wrote {
                f.write_str(separator)?;
            }
            f.write_str(name)?;
            wrote = true;
        }
    }
    if wrote {
        Ok(())
    } else {
        f.write_str("UNKNOWN")
    }
}
