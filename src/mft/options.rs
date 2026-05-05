//! Options for iterating over the Master File Table.

use std::num::NonZeroUsize;

use crate::{Usn, journal::DEFAULT_BUFFER_BYTES};

/// Maximum `USN_RECORD` major version the kernel is allowed to return.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum UsnRecordVersion {
    /// Force `USN_RECORD_V2` (standard 64-bit NTFS file IDs).
    V2,
    /// Permit `USN_RECORD_V3` (128-bit extended IDs, used on ReFS and some NTFS builds).
    V3,
}

impl UsnRecordVersion {
    /// Return the raw major-version value expected by the Windows API.
    pub(crate) const fn as_u16(self) -> u16 {
        match self {
            Self::V2 => 2,
            Self::V3 => 3,
        }
    }
}

/// Options for enumerating the Master File Table (MFT).
///
/// Allows customization of the USN range and buffer size for enumeration.
///
/// Use [`MftIterOptions::builder`] for the fluent builder API.
#[derive(Debug, Clone)]
pub struct MftIterOptions {
    /// Inclusive lower USN bound for returned records.
    pub(crate) low_usn: Usn,
    /// Inclusive upper USN bound for returned records.
    pub(crate) high_usn: Usn,
    /// Size of the kernel output buffer.
    pub(crate) buffer_bytes: NonZeroUsize,
    /// Highest `USN_RECORD` major version the kernel may return.
    pub(crate) max_usn_record_version: UsnRecordVersion,
}

impl Default for MftIterOptions {
    fn default() -> Self {
        MftIterOptions {
            low_usn: Usn::new(0),
            high_usn: Usn::new(i64::MAX),
            buffer_bytes: NonZeroUsize::new(DEFAULT_BUFFER_BYTES)
                .expect("default MFT buffer size is non-zero"),
            max_usn_record_version: UsnRecordVersion::V3,
        }
    }
}

impl MftIterOptions {
    /// Returns a fluent builder for [`MftIterOptions`].
    pub fn builder() -> MftIterOptionsBuilder {
        MftIterOptionsBuilder::default()
    }
}

/// Fluent builder for [`MftIterOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct MftIterOptionsBuilder {
    /// Mutable options value being configured by the builder.
    inner: MftIterOptions,
}

impl MftIterOptionsBuilder {
    /// Set the inclusive lower USN bound.
    pub fn low_usn(mut self, v: Usn) -> Self {
        self.inner.low_usn = v;
        self
    }

    /// Set the inclusive upper USN bound.
    pub fn high_usn(mut self, v: Usn) -> Self {
        self.inner.high_usn = v;
        self
    }

    /// Set the in-memory buffer size, in bytes.
    pub fn buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.buffer_bytes = v;
        self
    }

    /// Set the maximum `USN_RECORD` major version the kernel may return.
    ///
    /// Pass [`UsnRecordVersion::V2`] to force standard 64-bit NTFS file IDs.
    /// Pass [`UsnRecordVersion::V3`] (the default) to allow extended IDs.
    pub fn max_usn_record_version(mut self, v: UsnRecordVersion) -> Self {
        self.inner.max_usn_record_version = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> MftIterOptions {
        self.inner
    }
}
