//! Options for iterating over the Master File Table.

use crate::{Usn, journal::DEFAULT_BUFFER_BYTES};

/// Options for enumerating the Master File Table (MFT).
///
/// Allows customization of the USN range and buffer size for enumeration.
///
/// Use [`MftIterOptions::builder`] for the fluent builder API, or construct
/// directly via struct-literal syntax. [`Default`] is also implemented.
#[derive(Debug, Clone)]
pub struct MftIterOptions {
    pub low_usn: Usn,
    pub high_usn: Usn,
    pub buffer_size: usize,
}

impl Default for MftIterOptions {
    fn default() -> Self {
        MftIterOptions {
            low_usn: Usn::new(0),
            high_usn: Usn::new(i64::MAX),
            buffer_size: DEFAULT_BUFFER_BYTES,
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
    pub fn buffer_size(mut self, v: usize) -> Self {
        self.inner.buffer_size = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> MftIterOptions {
        self.inner
    }
}