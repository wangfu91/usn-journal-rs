//! `Mft` — high-level wrapper around `FSCTL_ENUM_USN_DATA`.

use crate::{UsnResult, errors::UsnError, volume::Volume};

use super::{iter::MftIter, options::MftIterOptions};

/// Represents the Master File Table (MFT) enumerator.
#[derive(Debug)]
pub struct Mft<'a> {
    pub(crate) volume: &'a Volume,
}

impl<'a> Mft<'a> {
    /// Creates a new `Mft` instance.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        Mft { volume }
    }

    /// Returns an iterator over the MFT entries.
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    pub fn try_iter(&self) -> UsnResult<MftIter> {
        self.try_iter_with_options(MftIterOptions::default())
    }

    /// Returns an iterator over the MFT entries with custom options.
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    pub fn try_iter_with_options(&self, options: MftIterOptions) -> UsnResult<MftIter> {
        if options.buffer_size == 0 {
            return Err(UsnError::InvalidOptions(
                "buffer_size must be greater than 0",
            ));
        }

        Ok(MftIter::new(
            self.volume.handle,
            options.low_usn.get(),
            options.high_usn.get(),
            options.max_usn_record_version,
            vec![0u8; options.buffer_size],
            options.low_usn.get() as u64,
        ))
    }
}