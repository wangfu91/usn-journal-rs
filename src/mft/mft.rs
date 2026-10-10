//! `Mft` — high-level wrapper around `FSCTL_ENUM_USN_DATA`.

use crate::volume::Volume;

use super::{iter::MftIter, options::MftIterOptions};

/// Represents the Master File Table (MFT) enumerator.
#[derive(Debug)]
pub struct Mft<'a> {
    /// Volume whose MFT will be enumerated.
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
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter(&self) -> MftIter {
        self.iter_with_options(MftIterOptions::default())
    }

    /// Returns an iterator over the MFT entries with custom options.
    ///
    /// The iterator yields `Result<MftEntry, UsnError>` items, allowing callers
    /// to handle individual entry errors gracefully without stopping iteration.
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn iter_with_options(&self, options: MftIterOptions) -> MftIter {
        self.iter_with_buffer(options, Vec::new())
    }

    /// Construct an iterator using a caller-owned reusable buffer.
    /// Resizes the buffer to the configured length before the first read.
    #[must_use]
    pub fn iter_with_buffer(&self, options: MftIterOptions, mut buffer: Vec<u8>) -> MftIter {
        buffer.resize(options.buffer_bytes.get(), 0);
        MftIter::new(
            self.volume.shared_handle(),
            options.low_usn.get(),
            options.high_usn.get(),
            options.max_usn_record_version.as_u16(),
            buffer,
        )
    }
}

impl Volume {
    /// Create an [`Mft`] enumerator for this volume.
    ///
    /// Convenience for [`Mft::new`], so you can write
    /// `volume.mft().iter()` without importing [`Mft`].
    #[must_use]
    pub fn mft(&self) -> Mft<'_> {
        Mft::new(self)
    }
}

impl IntoIterator for Mft<'_> {
    type Item = crate::UsnResult<super::MftEntry>;
    type IntoIter = MftIter;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_with_options(MftIterOptions::default())
    }
}
impl IntoIterator for &Mft<'_> {
    type Item = crate::UsnResult<super::MftEntry>;
    type IntoIter = MftIter;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_with_options(MftIterOptions::default())
    }
}
