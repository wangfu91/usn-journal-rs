//! USN journal state structure.

use crate::Usn;
use windows::Win32::System::Ioctl::USN_JOURNAL_DATA_V0;

/// Represents the USN journal state on an NTFS/ReFS volume.
/// This is a thin wrapper around the USN_JOURNAL_DATA_V0 structure from the Windows API.
#[derive(Debug, Clone)]
pub struct UsnJournalData {
    pub journal_id: u64,
    pub first_usn: Usn,
    pub next_usn: Usn,
    pub lowest_valid_usn: Usn,
    pub max_usn: Usn,
    pub maximum_size: u64,
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
