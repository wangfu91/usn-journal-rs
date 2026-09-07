//! Options for iterating over the Master File Table.

use std::num::NonZeroUsize;

use crate::{Usn, journal::DEFAULT_BUFFER_BYTES_NONZERO};

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
            buffer_bytes: DEFAULT_BUFFER_BYTES_NONZERO,
            max_usn_record_version: UsnRecordVersion::V3,
        }
    }
}

impl MftIterOptions {
    /// Return the inclusive lower USN bound for returned records.
    pub fn low_usn(&self) -> Usn {
        self.low_usn
    }

    /// Return the inclusive upper USN bound for returned records.
    pub fn high_usn(&self) -> Usn {
        self.high_usn
    }

    /// Return the kernel output buffer size in bytes.
    pub fn buffer_bytes(&self) -> NonZeroUsize {
        self.buffer_bytes
    }

    /// Return the highest `USN_RECORD` major version the kernel may return.
    pub fn max_usn_record_version(&self) -> UsnRecordVersion {
        self.max_usn_record_version
    }

    /// Consume these options to configure a builder with the same settings.
    ///
    /// Call [`MftIterOptionsBuilder::build`] to validate the updated options.
    /// Clone the options first if the original value is still needed.
    pub fn into_builder(self) -> MftIterOptionsBuilder {
        MftIterOptionsBuilder { inner: self }
    }
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
    /// Must fit the 8-byte cursor and the Win32 u32 length; build validates this.
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

    /// Validate the buffer size and USN range, then finalize the builder.
    pub fn build(self) -> crate::UsnResult<MftIterOptions> {
        crate::validate_buffer_bytes(self.inner.buffer_bytes.get())?;
        if self.inner.low_usn > self.inner.high_usn {
            return Err(crate::UsnError::InvalidOptions(
                "low_usn must not exceed high_usn",
            ));
        }
        Ok(self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spans_full_usn_range_and_allows_v3() {
        let opts = MftIterOptions::default();
        assert_eq!(opts.low_usn(), Usn::new(0));
        assert_eq!(opts.high_usn(), Usn::new(i64::MAX));
        assert_eq!(opts.max_usn_record_version(), UsnRecordVersion::V3);
    }

    #[test]
    fn usn_record_version_maps_to_major_version() {
        assert_eq!(UsnRecordVersion::V2.as_u16(), 2);
        assert_eq!(UsnRecordVersion::V3.as_u16(), 3);
    }

    #[test]
    fn builder_round_trips_values() {
        let opts = MftIterOptions::builder()
            .low_usn(Usn::new(10))
            .high_usn(Usn::new(20))
            .max_usn_record_version(UsnRecordVersion::V2)
            .buffer_bytes(NonZeroUsize::new(4 * 1024).unwrap())
            .build()
            .expect("valid iterator options");

        assert_eq!(opts.low_usn(), Usn::new(10));
        assert_eq!(opts.high_usn(), Usn::new(20));
        assert_eq!(opts.max_usn_record_version(), UsnRecordVersion::V2);
        assert_eq!(opts.buffer_bytes().get(), 4 * 1024);

        let updated = opts
            .clone()
            .into_builder()
            .high_usn(Usn::new(30))
            .build()
            .expect("valid rebuilt options");
        assert_eq!(updated.high_usn(), Usn::new(30));
        assert_eq!(updated.low_usn(), opts.low_usn());
        assert_eq!(updated.buffer_bytes(), opts.buffer_bytes());
        assert_eq!(
            updated.max_usn_record_version(),
            opts.max_usn_record_version()
        );
        assert!(
            updated
                .clone()
                .into_builder()
                .low_usn(Usn::new(31))
                .build()
                .is_err()
        );
        assert!(
            updated
                .into_builder()
                .buffer_bytes(NonZeroUsize::new(7).unwrap())
                .build()
                .is_err()
        );
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    #[test]
    fn validates_inclusive_ranges() {
        assert!(
            MftIterOptions::builder()
                .low_usn(Usn::new(2))
                .high_usn(Usn::new(1))
                .build()
                .is_err()
        );
        assert!(
            MftIterOptions::builder()
                .low_usn(Usn::ZERO)
                .high_usn(Usn::ZERO)
                .build()
                .is_ok()
        );
        assert!(
            MftIterOptions::builder()
                .buffer_bytes(NonZeroUsize::new(1).unwrap())
                .build()
                .is_err()
        );
    }
}
