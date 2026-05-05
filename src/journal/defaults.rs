//! Default constants used by the USN journal module.

use std::num::NonZeroUsize;

/// Default in-memory buffer size used when reading USN journal records.
pub const DEFAULT_BUFFER_BYTES: usize = 64 * 1024; // 64KB

/// Default in-memory buffer size as a non-zero value.
#[allow(clippy::useless_nonzero_new_unchecked)]
pub const DEFAULT_BUFFER_BYTES_NONZERO: NonZeroUsize = unsafe {
    // SAFETY: `64 * 1024` is a non-zero constant.
    NonZeroUsize::new_unchecked(DEFAULT_BUFFER_BYTES)
};

/// Default maximum size, in bytes, used when creating a USN journal.
pub const DEFAULT_JOURNAL_MAX_SIZE: u64 = 32 * 1024 * 1024; // 32MB

/// Default allocation delta, in bytes, used when creating a USN journal.
pub const DEFAULT_JOURNAL_ALLOCATION_DELTA: u64 = 8 * 1024 * 1024; // 8MB

/// Reason mask that matches every USN reason flag.
pub const USN_REASON_MASK_ALL: u32 = 0xFFFFFFFF;
