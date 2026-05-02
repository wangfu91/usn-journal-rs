use std::num::NonZeroUsize;

use crate::raw_mft::{DEFAULT_BUFFER_BYTES, record::FIRST_NORMAL_RECORD};

/// Options controlling iteration behaviour.
///
/// Use [`RawMftIterOptions::builder`] for the fluent builder API.
#[derive(Debug, Clone)]
pub struct RawMftIterOptions {
    /// Size of the I/O buffer in bytes used for batched reads of FILE records.
    pub(crate) buffer_bytes: NonZeroUsize,
    /// Honour the `$MFT` `$BITMAP` to skip unused records.
    pub(crate) skip_unused: bool,
    /// First record number to yield.
    pub(crate) start_record: u64,
    /// Last record number to yield (exclusive); `None` means up to the
    /// total number of MFT records.
    pub(crate) end_record: Option<u64>,
}

impl Default for RawMftIterOptions {
    fn default() -> Self {
        Self {
            buffer_bytes: DEFAULT_BUFFER_BYTES,
            skip_unused: true,
            start_record: FIRST_NORMAL_RECORD,
            end_record: None,
        }
    }
}

impl RawMftIterOptions {
    /// Returns a fluent builder for [`RawMftIterOptions`].
    pub fn builder() -> RawMftIterOptionsBuilder {
        RawMftIterOptionsBuilder::default()
    }
}

/// Fluent builder for [`RawMftIterOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct RawMftIterOptionsBuilder {
    inner: RawMftIterOptions,
}

impl RawMftIterOptionsBuilder {
    /// Set the I/O buffer size in bytes.
    pub fn buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.buffer_bytes = v;
        self
    }

    /// Whether to honour the `$MFT` `$BITMAP` and skip unused records.
    pub fn skip_unused(mut self, v: bool) -> Self {
        self.inner.skip_unused = v;
        self
    }

    /// Set the inclusive starting record number.
    pub fn start_record(mut self, v: u64) -> Self {
        self.inner.start_record = v;
        self
    }

    /// Set the exclusive end record number, or `None` to iterate the full MFT.
    pub fn end_record(mut self, v: Option<u64>) -> Self {
        self.inner.end_record = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> RawMftIterOptions {
        self.inner
    }
}
