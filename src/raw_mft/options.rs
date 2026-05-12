//! Builder-configurable options for raw `$MFT` scans.

use std::num::NonZeroUsize;

use crate::raw_mft::{
    DEFAULT_ATTR_BUFFER_BYTES, DEFAULT_BUFFER_BYTES, layout::record::FIRST_NORMAL_RECORD,
};

/// Inclusive/exclusive logical record range for a raw `$MFT` scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMftRecordRange {
    /// First record number to include.
    pub(crate) start_record: u64,
    /// Exclusive end record number; `None` means up to `RawMft::record_count()`.
    pub(crate) end_record: Option<u64>,
}

impl Default for RawMftRecordRange {
    fn default() -> Self {
        Self {
            start_record: FIRST_NORMAL_RECORD,
            end_record: None,
        }
    }
}

impl RawMftRecordRange {
    /// Build a range from an inclusive start and optional exclusive end.
    #[must_use]
    pub const fn new(start_record: u64, end_record: Option<u64>) -> Self {
        Self {
            start_record,
            end_record,
        }
    }

    /// Inclusive first record number.
    #[must_use]
    pub const fn start_record(&self) -> u64 {
        self.start_record
    }

    /// Exclusive end record number, or `None` for the full `$MFT`.
    #[must_use]
    pub const fn end_record(&self) -> Option<u64> {
        self.end_record
    }
}

/// Buffer sizes used by raw `$MFT` scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMftReadBuffers {
    /// Size of the I/O buffer in bytes used for batched reads of FILE records.
    pub(crate) main: NonZeroUsize,
    /// Size of the I/O buffer in bytes used for random attribute-list extension reads.
    pub(crate) attr: NonZeroUsize,
}

impl Default for RawMftReadBuffers {
    fn default() -> Self {
        Self {
            main: DEFAULT_BUFFER_BYTES,
            attr: DEFAULT_ATTR_BUFFER_BYTES,
        }
    }
}

impl RawMftReadBuffers {
    /// Build explicit main and attribute-list read buffers.
    #[must_use]
    pub const fn new(main: NonZeroUsize, attr: NonZeroUsize) -> Self {
        Self { main, attr }
    }

    /// Main sequential FILE-record scan buffer size in bytes.
    #[must_use]
    pub const fn main(&self) -> NonZeroUsize {
        self.main
    }

    /// Random extension-record read buffer size in bytes.
    #[must_use]
    pub const fn attr(&self) -> NonZeroUsize {
        self.attr
    }
}

/// Metadata materialization choices for raw `$MFT` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMftEntryOptions {
    /// When false, skip collecting named alternate data streams.
    pub(crate) collect_alternate_data_streams: bool,
    /// When false, skip summarizing non-resident data runs.
    pub(crate) collect_data_run_summary: bool,
    /// When false, omit DOS 8.3 `$FILE_NAME` links shadowed by a better same-parent name.
    pub(crate) collect_dos_file_name_links: bool,
}

impl Default for RawMftEntryOptions {
    fn default() -> Self {
        Self {
            collect_alternate_data_streams: true,
            collect_data_run_summary: true,
            collect_dos_file_name_links: true,
        }
    }
}

impl RawMftEntryOptions {
    /// Whether named alternate data streams are collected.
    #[must_use]
    pub const fn collect_alternate_data_streams(&self) -> bool {
        self.collect_alternate_data_streams
    }

    /// Whether non-resident unnamed data computes a run summary.
    #[must_use]
    pub const fn collect_data_run_summary(&self) -> bool {
        self.collect_data_run_summary
    }

    /// Whether shadowed DOS 8.3 `$FILE_NAME` links are retained.
    #[must_use]
    pub const fn collect_dos_file_name_links(&self) -> bool {
        self.collect_dos_file_name_links
    }
}

/// Options controlling raw `$MFT` scan behavior.
///
/// Use [`RawMftScanOptions::builder`] for the fluent builder API.
#[derive(Debug, Clone)]
pub struct RawMftScanOptions {
    /// Read-buffer sizing.
    pub(crate) buffers: RawMftReadBuffers,
    /// Logical record range.
    pub(crate) range: RawMftRecordRange,
    /// Entry materialization choices.
    pub(crate) entry: RawMftEntryOptions,
    /// Honor the `$MFT` `$BITMAP` to skip unused records.
    pub(crate) skip_unused: bool,
    /// When true, omit extension (non-base) records from the yielded stream.
    ///
    /// An extension record is an overflow FILE record whose `base_reference`
    /// header field points back to the base record.  A base record has
    /// `base_reference == 0` and represents one unique file or directory.
    /// Filtering out extension records yields exactly one entry per file or
    /// directory — the same view Windows Explorer shows.
    ///
    /// Defaults to `true`.  Set to `false` only if you explicitly need to
    /// inspect raw extension record contents.
    pub(crate) skip_extension_records: bool,
}

impl Default for RawMftScanOptions {
    fn default() -> Self {
        Self {
            buffers: RawMftReadBuffers::default(),
            range: RawMftRecordRange::default(),
            entry: RawMftEntryOptions::default(),
            skip_unused: true,
            skip_extension_records: true,
        }
    }
}

impl RawMftScanOptions {
    /// Returns a fluent builder for [`RawMftScanOptions`].
    pub fn builder() -> RawMftScanOptionsBuilder {
        RawMftScanOptionsBuilder::default()
    }

    /// Read-buffer sizing.
    #[must_use]
    pub const fn buffers(&self) -> RawMftReadBuffers {
        self.buffers
    }

    /// Logical record range.
    #[must_use]
    pub const fn range(&self) -> RawMftRecordRange {
        self.range
    }

    /// Entry materialization choices.
    #[must_use]
    pub const fn entry(&self) -> RawMftEntryOptions {
        self.entry
    }

    /// Whether unused records are skipped using the `$MFT` bitmap.
    #[must_use]
    pub const fn skip_unused(&self) -> bool {
        self.skip_unused
    }

    /// Whether extension records are omitted from the yielded stream.
    #[must_use]
    pub const fn skip_extension_records(&self) -> bool {
        self.skip_extension_records
    }
}

/// Fluent builder for [`RawMftScanOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct RawMftScanOptionsBuilder {
    /// Mutable options value being configured by the builder.
    inner: RawMftScanOptions,
}

impl RawMftScanOptionsBuilder {
    /// Set the I/O buffer size in bytes.
    pub fn buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.buffers.main = v;
        self
    }

    /// Set the random attribute-list extension-read buffer size in bytes.
    pub fn attr_buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.buffers.attr = v;
        self
    }

    /// Set both read-buffer sizes at once.
    pub fn buffers(mut self, v: RawMftReadBuffers) -> Self {
        self.inner.buffers = v;
        self
    }

    /// Set the logical scan range.
    pub fn range(mut self, v: RawMftRecordRange) -> Self {
        self.inner.range = v;
        self
    }

    /// Whether to honor the `$MFT` `$BITMAP` and skip unused records.
    pub fn skip_unused(mut self, v: bool) -> Self {
        self.inner.skip_unused = v;
        self
    }

    /// Whether extension (non-base) records should be omitted from the yielded stream.
    ///
    /// Defaults to `true`.  See [`RawMftScanOptions::skip_extension_records`] for details.
    pub fn skip_extension_records(mut self, v: bool) -> Self {
        self.inner.skip_extension_records = v;
        self
    }

    /// Whether named alternate data streams should be collected.
    pub fn collect_alternate_data_streams(mut self, v: bool) -> Self {
        self.inner.entry.collect_alternate_data_streams = v;
        self
    }

    /// Whether non-resident unnamed data should compute a run summary.
    pub fn collect_data_run_summary(mut self, v: bool) -> Self {
        self.inner.entry.collect_data_run_summary = v;
        self
    }

    /// Whether DOS 8.3 `$FILE_NAME` links shadowed by better same-parent names should be collected.
    pub fn collect_dos_file_name_links(mut self, v: bool) -> Self {
        self.inner.entry.collect_dos_file_name_links = v;
        self
    }

    /// Set all entry materialization choices at once.
    pub fn entry(mut self, v: RawMftEntryOptions) -> Self {
        self.inner.entry = v;
        self
    }

    /// Set the inclusive starting record number.
    pub fn start_record(mut self, v: u64) -> Self {
        self.inner.range.start_record = v;
        self
    }

    /// Set the exclusive end record number, or `None` to iterate the full MFT.
    pub fn end_record(mut self, v: Option<u64>) -> Self {
        self.inner.range.end_record = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> RawMftScanOptions {
        self.inner
    }
}
