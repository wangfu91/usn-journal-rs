//! USN journal state structure.

use crate::Usn;
use std::fmt;
use windows::Win32::System::Ioctl::USN_JOURNAL_DATA_V0;

/// Represents the USN journal state on an NTFS/ReFS volume.
/// This is a thin wrapper around the USN_JOURNAL_DATA_V0 structure from the Windows API.
#[derive(Debug, Clone)]
pub struct UsnJournalData {
    /// Opaque identifier of the current journal instance.
    pub journal_id: u64,
    /// Lowest USN currently present in the journal.
    pub first_usn: Usn,
    /// USN that will be assigned to the next journal record.
    pub next_usn: Usn,
    /// Lowest USN that can still be queried reliably.
    pub lowest_valid_usn: Usn,
    /// Maximum USN the journal can reach before rollover handling.
    pub max_usn: Usn,
    /// Target maximum size of the journal in bytes.
    pub maximum_size: u64,
    /// Allocation quantum used when growing the journal.
    pub allocation_delta: u64,
}

impl From<USN_JOURNAL_DATA_V0> for UsnJournalData {
    fn from(data: USN_JOURNAL_DATA_V0) -> Self {
        UsnJournalData {
            journal_id: data.UsnJournalID,
            first_usn: Usn::new(data.FirstUsn),
            next_usn: Usn::new(data.NextUsn),
            lowest_valid_usn: Usn::new(data.LowestValidUsn),
            max_usn: Usn::new(data.MaxUsn),
            maximum_size: data.MaximumSize,
            allocation_delta: data.AllocationDelta,
        }
    }
}

impl fmt::Display for UsnJournalData {
    /// Compact one-line summary suitable for logging.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "journal 0x{:x}: usn {}..{} (lowest_valid {}), max_size {} bytes, delta {} bytes",
            self.journal_id,
            self.first_usn,
            self.next_usn,
            self.lowest_valid_usn,
            self.maximum_size,
            self.allocation_delta,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UsnJournalData {
        UsnJournalData {
            journal_id: 0xABCD,
            first_usn: Usn::new(0x1000),
            next_usn: Usn::new(0x5000),
            lowest_valid_usn: Usn::new(0x800),
            max_usn: Usn::new(0x10_000),
            maximum_size: 32 * 1024 * 1024,
            allocation_delta: 8 * 1024 * 1024,
        }
    }

    #[test]
    fn from_win32_maps_all_fields() {
        let raw = USN_JOURNAL_DATA_V0 {
            UsnJournalID: 0xABCD,
            FirstUsn: 0x1000,
            NextUsn: 0x5000,
            LowestValidUsn: 0x800,
            MaxUsn: 0x10_000,
            MaximumSize: 32 * 1024 * 1024,
            AllocationDelta: 8 * 1024 * 1024,
        };
        let data = UsnJournalData::from(raw);
        assert_eq!(data.journal_id, 0xABCD);
        assert_eq!(data.first_usn, Usn::new(0x1000));
        assert_eq!(data.next_usn, Usn::new(0x5000));
        assert_eq!(data.lowest_valid_usn, Usn::new(0x800));
        assert_eq!(data.max_usn, Usn::new(0x10_000));
    }

    #[test]
    fn display_is_compact_summary() {
        let text = sample().to_string();
        assert!(text.contains("journal 0xabcd"));
        assert!(text.contains("usn 4096..20480"));
        assert!(text.contains("lowest_valid 2048"));
    }
}
