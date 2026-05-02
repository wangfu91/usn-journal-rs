//! Small shared helpers for path resolution.

use crate::Fid;

/// NTFS root directory MFT record number (`$Root`).
///
/// In parent-chain walks, reaching this record means path resolution has
/// reached the filesystem root and should stop climbing.
pub(crate) const NTFS_ROOT_RECORD_NUMBER: u64 = 5;

/// Mask a standard 64-bit NTFS file reference number to its 48-bit record
/// number portion (clearing the 16-bit sequence number in the high bits).
pub(crate) fn mask_fid_to_record_number(fid: Fid) -> Option<u64> {
    fid.record_number()
}
