//! Builder-configurable options for raw `$MFT` iteration.

use std::num::NonZeroUsize;

use crate::raw_mft::{
    DEFAULT_ATTR_BUFFER_BYTES, DEFAULT_BUFFER_BYTES, record::FIRST_NORMAL_RECORD,
};

/// Options controlling iteration behaviour.
///
/// Use [`RawMftIterOptions::builder`] for the fluent builder API.
#[derive(Debug, Clone)]
pub struct RawMftIterOptions {
    /// Size of the I/O buffer in bytes used for batched reads of FILE records.
    pub(crate) buffer_bytes: NonZeroUsize,
    /// Size of the I/O buffer in bytes used for random attribute-list extension reads.
    pub(crate) attr_buffer_bytes: NonZeroUsize,
    /// Honour the `$MFT` `$BITMAP` to skip unused records.
    pub(crate) skip_unused: bool,
    /// When true, omit extension records from the yielded stream.
    pub(crate) skip_extension_records: bool,
    /// When false, skip collecting named alternate data streams.
    pub(crate) collect_alternate_data_streams: bool,
    /// When false, skip summarizing non-resident data runs.
    pub(crate) collect_data_run_summary: bool,
    /// When false, omit DOS 8.3 `$FILE_NAME` links shadowed by a better same-parent name.
    pub(crate) collect_dos_file_name_links: bool,
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
            attr_buffer_bytes: DEFAULT_ATTR_BUFFER_BYTES,
            skip_unused: true,
            skip_extension_records: false,
            collect_alternate_data_streams: true,
            collect_data_run_summary: true,
            collect_dos_file_name_links: true,
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
    /// Mutable options value being configured by the builder.
    inner: RawMftIterOptions,
}

impl RawMftIterOptionsBuilder {
    /// Set the I/O buffer size in bytes.
    pub fn buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.buffer_bytes = v;
        self
    }

    /// Set the random attribute-list extension-read buffer size in bytes.
    pub fn attr_buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.attr_buffer_bytes = v;
        self
    }

    /// Whether to honour the `$MFT` `$BITMAP` and skip unused records.
    pub fn skip_unused(mut self, v: bool) -> Self {
        self.inner.skip_unused = v;
        self
    }

    /// Whether extension records should be omitted from the yielded stream.
    pub fn skip_extension_records(mut self, v: bool) -> Self {
        self.inner.skip_extension_records = v;
        self
    }

    /// Whether named alternate data streams should be collected.
    pub fn collect_alternate_data_streams(mut self, v: bool) -> Self {
        self.inner.collect_alternate_data_streams = v;
        self
    }

    /// Whether non-resident unnamed data should compute a run summary.
    pub fn collect_data_run_summary(mut self, v: bool) -> Self {
        self.inner.collect_data_run_summary = v;
        self
    }

    /// Whether DOS 8.3 `$FILE_NAME` links shadowed by better same-parent names should be collected.
    pub fn collect_dos_file_name_links(mut self, v: bool) -> Self {
        self.inner.collect_dos_file_name_links = v;
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
