//! USN journal state structure.

use crate::Usn;
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
