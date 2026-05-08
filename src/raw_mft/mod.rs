//! Raw `$MFT` reader for NTFS volumes.
//!
//! This module reads the `$MFT` file directly from the volume and parses
//! each FILE record to expose rich per-record metadata that the USN-based
//! [`crate::mft::Mft`] enumerator cannot surface (full timestamps, real
//! and allocated size, hard link count, alternate data streams, sparse
//! / compressed flags, data-run summary, file-name namespace, etc.).
//!
//! Quick start:
//!
//! ```no_run
//! use usn_journal_rs::{volume::Volume, raw_mft::RawMft};
//!
//! let volume = Volume::from_drive_letter('C').expect("open volume");
//! let mft = RawMft::new(&volume).expect("read $MFT");
//! for entry in mft.iter().expect("iter") {
//!     match entry {
//!         Ok(e) if e.is_used => {
//!             println!("{:>8}: {}", e.record_number, e.file_name.to_string_lossy());
//!         }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! ## Limitations
//!
//! * NTFS only — ReFS volumes return [`crate::errors::UsnError::UnsupportedFilesystem`].
//! * `$ATTRIBUTE_LIST` enrichment is intentionally one level deep; extension
//!   records are loaded when needed but are not recursively enriched.
//! * Reading the volume requires Administrator privileges.

mod attr_list;
mod bootstrap;
mod chunk_plan;
mod entry_build;
/// Hidden support helpers shared by the raw-MFT ingest benchmark and tooling examples.
#[doc(hidden)]
pub mod ingest_support;
mod io;
mod ondisk;
mod options;
mod parallel;
mod reader;
mod serial;
#[cfg(test)]
mod tests;

use std::num::NonZeroUsize;
use std::sync::Arc;

use crate::{
    errors::UsnError,
    raw_mft::{
        entry_build::EntryBuildOptions,
        io::VolumeReader,
        ondisk::{boot::BootSector, extent::ExtentMap},
        reader::read_record_at,
    },
    volume::Volume,
};

pub use chunk_plan::{RawMftChunkPlanOptions, RawMftChunkPlanOptionsBuilder, RawMftWorkChunk};
pub use entry_build::{AdsInfo, RawMftBatchEntry, RawMftChunkBatch, RawMftEntry, RawMftLink};
pub use ondisk::attribute::FileNameNamespace;
pub use ondisk::data_run::{DataRun as DataRunInfo, DataRunSummary};
pub use options::{
    RawMftEntryOptions, RawMftReadBuffers, RawMftRecordRange, RawMftScanOptions,
    RawMftScanOptionsBuilder,
};
pub use parallel::RawMftParallelScan;
pub use serial::RawMftIter;

/// Default I/O buffer size for raw `$MFT` iteration.
#[allow(clippy::useless_nonzero_new_unchecked)]
pub const DEFAULT_BUFFER_BYTES: NonZeroUsize = unsafe {
    // SAFETY: `256 * 1024` is a non-zero constant.
    NonZeroUsize::new_unchecked(256 * 1024)
};
/// Default buffer size for `$ATTRIBUTE_LIST` and extension-record reads.
///
/// These reads are typically smaller and less sequential than the main `$MFT`
/// scan, so the attribute buffer is tuned independently from
/// [`DEFAULT_BUFFER_BYTES`].
#[allow(clippy::useless_nonzero_new_unchecked)]
pub const DEFAULT_ATTR_BUFFER_BYTES: NonZeroUsize = unsafe {
    // SAFETY: `64 * 1024` is a non-zero constant.
    NonZeroUsize::new_unchecked(64 * 1024)
};

/// Raw `$MFT` reader bound to an open [`Volume`].
pub struct RawMft<'a> {
    /// Volume whose `$MFT` is being read.
    volume: &'a Volume,
    /// Parsed NTFS boot-sector geometry.
    boot: BootSector,
    /// Shared extent map for the `$MFT` data stream.
    extent_map: Arc<ExtentMap>,
    /// Shared contents of the `$MFT` bitmap stream when available.
    bitmap: Arc<[u8]>,
}

impl<'a> RawMft<'a> {
    /// Total number of FILE records this MFT can address.
    #[must_use]
    #[inline]
    pub fn record_count(&self) -> u64 {
        self.extent_map.record_count()
    }

    /// Cluster size in bytes.
    #[must_use]
    #[inline]
    pub fn cluster_size(&self) -> u64 {
        self.boot.cluster_size
    }

    /// File record size in bytes.
    #[must_use]
    #[inline]
    pub fn file_record_size(&self) -> u64 {
        self.boot.file_record_size
    }

    /// Read a single record by number. Returns `Ok(None)` when the
    /// record falls in a sparse hole or is unused (and `skip_unused` is
    /// implied here).
    pub fn read_record(&self, number: u64) -> Result<Option<RawMftEntry>, UsnError> {
        let mut reader = VolumeReader::new(self.volume.handle, self.boot.bytes_per_sector as u64)?;
        read_record_at(
            &mut reader,
            &self.boot,
            self.extent_map.as_ref(),
            number,
            EntryBuildOptions::full(),
        )
    }

    /// True if `record_number` is marked as in-use in the `$BITMAP`.
    /// Returns `true` when no bitmap is available.
    pub fn bitmap_used(&self, record_number: u64) -> bool {
        if self.bitmap.is_empty() {
            return true;
        }
        let byte = (record_number / 8) as usize;
        let bit = (record_number % 8) as u8;
        match self.bitmap.get(byte) {
            Some(&b) => (b >> bit) & 1 != 0,
            None => false,
        }
    }
}
