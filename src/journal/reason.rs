//! Helpers for formatting USN reason bitfields.

use std::fmt;

use crate::UsnReason;
use crate::display::write_flag_names;

/// Display names for known `USN_REASON_*` bits.
const REASON_FLAG_NAMES: &[(u32, &str)] = &[
    (UsnReason::DATA_OVERWRITE.bits(), "DATA_OVERWRITE"),
    (UsnReason::DATA_EXTEND.bits(), "DATA_EXTEND"),
    (UsnReason::DATA_TRUNCATION.bits(), "DATA_TRUNCATION"),
    (
        UsnReason::NAMED_DATA_OVERWRITE.bits(),
        "NAMED_DATA_OVERWRITE",
    ),
    (UsnReason::NAMED_DATA_EXTEND.bits(), "NAMED_DATA_EXTEND"),
    (
        UsnReason::NAMED_DATA_TRUNCATION.bits(),
        "NAMED_DATA_TRUNCATION",
    ),
    (UsnReason::FILE_CREATE.bits(), "FILE_CREATE"),
    (UsnReason::FILE_DELETE.bits(), "FILE_DELETE"),
    (UsnReason::EA_CHANGE.bits(), "EA_CHANGE"),
    (UsnReason::SECURITY_CHANGE.bits(), "SECURITY_CHANGE"),
    (UsnReason::RENAME_OLD_NAME.bits(), "RENAME_OLD_NAME"),
    (UsnReason::RENAME_NEW_NAME.bits(), "RENAME_NEW_NAME"),
    (UsnReason::INDEXABLE_CHANGE.bits(), "INDEXABLE_CHANGE"),
    (UsnReason::BASIC_INFO_CHANGE.bits(), "BASIC_INFO_CHANGE"),
    (UsnReason::HARD_LINK_CHANGE.bits(), "HARD_LINK_CHANGE"),
    (UsnReason::COMPRESSION_CHANGE.bits(), "COMPRESSION_CHANGE"),
    (UsnReason::ENCRYPTION_CHANGE.bits(), "ENCRYPTION_CHANGE"),
    (UsnReason::OBJECT_ID_CHANGE.bits(), "OBJECT_ID_CHANGE"),
    (
        UsnReason::REPARSE_POINT_CHANGE.bits(),
        "REPARSE_POINT_CHANGE",
    ),
    (UsnReason::STREAM_CHANGE.bits(), "STREAM_CHANGE"),
    (UsnReason::TRANSACTED_CHANGE.bits(), "TRANSACTED_CHANGE"),
    (UsnReason::INTEGRITY_CHANGE.bits(), "INTEGRITY_CHANGE"),
    (
        UsnReason::DESIRED_STORAGE_CLASS_CHANGE.bits(),
        "DESIRED_STORAGE_CLASS_CHANGE",
    ),
    (UsnReason::CLOSE.bits(), "CLOSE"),
];

/// Compact formatter for `UsnReason` that omits spaces around separators.
pub(super) struct CompactReason(pub(super) UsnReason);

impl fmt::Display for CompactReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_flag_names(f, self.0.bits(), REASON_FLAG_NAMES, "|")
    }
}

impl fmt::Display for UsnReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_flag_names(f, self.bits(), REASON_FLAG_NAMES, " | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_none_for_empty() {
        assert_eq!(UsnReason::empty().to_string(), "NONE");
    }

    #[test]
    fn display_lists_known_flags_with_separator() {
        let reason = UsnReason::FILE_CREATE | UsnReason::CLOSE;
        assert_eq!(reason.to_string(), "FILE_CREATE | CLOSE");
    }

    #[test]
    fn compact_display_omits_spaces() {
        let reason = UsnReason::FILE_CREATE | UsnReason::CLOSE;
        assert_eq!(CompactReason(reason).to_string(), "FILE_CREATE|CLOSE");
    }

    #[test]
    fn display_uses_hex_for_unknown_bits() {
        let reason = UsnReason::from_bits_retain(0x0000_0008);
        assert_eq!(reason.to_string(), "0x8");
    }
}
