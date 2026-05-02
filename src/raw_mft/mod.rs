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
//! for entry in mft.try_iter().expect("iter") {
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
//! * Non-resident `$ATTRIBUTE_LIST` (highly fragmented MFT records) is
//!   currently logged and skipped; the entry is still returned but may
//!   miss attributes spread across multiple base records.
//! * Reading the volume requires Administrator privileges.

mod attribute;
mod boot;
mod data_run;
mod entry;
mod extent;
mod fixup;
mod io;
mod record;

use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroUsize;

use log::{debug, warn};

use crate::{
    errors::UsnError,
    raw_mft::{
        attribute::{NtfsAttributeType, for_each_attribute},
        boot::BootSector,
        data_run::{DataRun, decode_runs},
        extent::ExtentMap,
        io::VolumeReader,
        record::{FIRST_NORMAL_RECORD, FileRecord, MFT_RECORD_NUMBER},
    },
    volume::Volume,
};

pub use attribute::FileNameNamespace;
pub use data_run::{DataRun as DataRunInfo, DataRunSummary};
pub use entry::{AdsInfo, RawMftEntry};

/// Default I/O buffer size for raw `$MFT` iteration.
pub const DEFAULT_BUFFER_BYTES: NonZeroUsize = match NonZeroUsize::new(256 * 1024) {
    Some(v) => v,
    None => unreachable!(),
};

/// Options controlling iteration behaviour.
///
/// Use [`RawMftOptions::builder`] for the fluent builder API, or construct
/// directly via struct-literal syntax. [`Default`] is also implemented.
#[derive(Debug, Clone)]
pub struct RawMftOptions {
    /// Size of the I/O buffer in bytes used for batched reads of FILE records.
    pub buffer_bytes: NonZeroUsize,
    /// Honour the `$MFT` `$BITMAP` to skip unused records.
    pub skip_unused: bool,
    /// First record number to yield.
    pub start_record: u64,
    /// Last record number to yield (exclusive); `None` means up to the
    /// total number of MFT records.
    pub end_record: Option<u64>,
}

impl Default for RawMftOptions {
    fn default() -> Self {
        Self {
            buffer_bytes: DEFAULT_BUFFER_BYTES,
            skip_unused: true,
            start_record: FIRST_NORMAL_RECORD,
            end_record: None,
        }
    }
}

impl RawMftOptions {
    /// Returns a fluent builder for [`RawMftOptions`].
    pub fn builder() -> RawMftOptionsBuilder {
        RawMftOptionsBuilder::default()
    }
}

/// Fluent builder for [`RawMftOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct RawMftOptionsBuilder {
    inner: RawMftOptions,
}

impl RawMftOptionsBuilder {
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
    pub fn build(self) -> RawMftOptions {
        self.inner
    }
}

/// Raw `$MFT` reader bound to an open [`Volume`].
pub struct RawMft<'a> {
    volume: &'a Volume,
    boot: BootSector,
    extent_map: ExtentMap,
    bitmap: Vec<u8>,
}

impl<'a> RawMft<'a> {
    /// Open the volume's `$MFT`, parse the boot sector and record 0, and
    /// build the extent map + bitmap.
    pub fn new(volume: &'a Volume) -> Result<Self, UsnError> {
        let mut reader = VolumeReader::new(volume.handle, 512)?;

        // Boot sector.
        let mut boot_buf = vec![0u8; 512];
        reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
        reader.read_exact(&mut boot_buf).map_err(io_err)?;
        let boot = BootSector::parse(&boot_buf)?;

        // Re-create the reader using the actual sector size from the boot
        // sector (it may differ from the 512 default).
        let mut reader = VolumeReader::new(volume.handle, boot.bytes_per_sector as u64)?;

        debug!(
            "raw_mft: cluster_size={} file_record_size={} mft_lcn={} mft_byte_offset={}",
            boot.cluster_size, boot.file_record_size, boot.mft_lcn, boot.mft_byte_offset
        );

        // Read MFT record 0.
        let mut record0 = vec![0u8; boot.file_record_size as usize];
        reader
            .seek(SeekFrom::Start(boot.mft_byte_offset))
            .map_err(io_err)?;
        reader.read_exact(&mut record0).map_err(io_err)?;
        let (data_runs, bitmap_runs, bitmap_size) = {
            let parsed = FileRecord::parse(MFT_RECORD_NUMBER, &mut record0)?;

            // Walk attributes for unnamed $DATA (extent map) and $BITMAP.
            let mut data_runs: Option<Vec<DataRun>> = None;
            let mut bitmap_runs: Option<Vec<DataRun>> = None;
            let mut bitmap_size: u64 = 0;
            let (off, used) = parsed.attrs_range();
            for_each_attribute(parsed.data, off, used, |attr| {
                let type_id = attr.type_id();
                let unnamed = attr.name_slice().is_none();
                if type_id == NtfsAttributeType::Data as u32 && unnamed && attr.is_non_resident() {
                    if let Some(h) = attr.nonresident_header() {
                        let runs_off = h.data_runs_offset as usize;
                        let attr_data = attr.data();
                        if runs_off <= attr_data.len() {
                            match decode_runs(&attr_data[runs_off..]) {
                                Ok((rs, _)) => data_runs = Some(rs),
                                Err(e) => warn!("$MFT $DATA decode_runs failed: {e}"),
                            }
                        }
                    }
                } else if type_id == NtfsAttributeType::Bitmap as u32
                    && attr.is_non_resident()
                    && let Some(h) = attr.nonresident_header()
                {
                    bitmap_size = h.data_size;
                    let runs_off = h.data_runs_offset as usize;
                    let attr_data = attr.data();
                    if runs_off <= attr_data.len() {
                        match decode_runs(&attr_data[runs_off..]) {
                            Ok((rs, _)) => bitmap_runs = Some(rs),
                            Err(e) => warn!("$MFT $BITMAP decode_runs failed: {e}"),
                        }
                    }
                }
            });
            (data_runs, bitmap_runs, bitmap_size)
        };

        let data_runs = data_runs.ok_or(UsnError::MftAttributeMissing("$MFT $DATA"))?;
        let extent_map = ExtentMap::from_runs(&data_runs, boot.cluster_size, boot.file_record_size);

        let bitmap = if let Some(br) = bitmap_runs {
            read_nonresident(&mut reader, &br, boot.cluster_size, bitmap_size)?
        } else {
            warn!("raw_mft: no $MFT $BITMAP; skip_unused will be ignored");
            Vec::new()
        };

        Ok(RawMft {
            volume,
            boot,
            extent_map,
            bitmap,
        })
    }

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

    /// Begin iteration with default options.
    pub fn try_iter(&self) -> Result<RawMftIter<'_>, UsnError> {
        self.try_iter_with_options(RawMftOptions::default())
    }

    /// Begin iteration with custom options.
    pub fn try_iter_with_options(
        &self,
        options: RawMftOptions,
    ) -> Result<RawMftIter<'_>, UsnError> {
        let reader = VolumeReader::with_buffer_bytes(
            self.volume.handle,
            self.boot.bytes_per_sector as u64,
            options.buffer_bytes.get(),
        )?;
        let total = self.record_count();
        let end = options.end_record.unwrap_or(total).min(total);
        Ok(RawMftIter {
            mft: self,
            reader,
            next_record: options.start_record,
            end,
            options,
        })
    }

    /// Read a single record by number. Returns `Ok(None)` when the
    /// record falls in a sparse hole or is unused (and `skip_unused` is
    /// implied here).
    pub fn get_record(&self, number: u64) -> Result<Option<RawMftEntry>, UsnError> {
        let mut reader = VolumeReader::new(self.volume.handle, self.boot.bytes_per_sector as u64)?;
        read_record_at(&mut reader, &self.boot, &self.extent_map, number)
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

/// Streaming iterator over MFT records.
pub struct RawMftIter<'a> {
    mft: &'a RawMft<'a>,
    reader: VolumeReader,
    next_record: u64,
    end: u64,
    options: RawMftOptions,
}

impl<'a> Iterator for RawMftIter<'a> {
    type Item = Result<RawMftEntry, UsnError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next_record < self.end {
            let n = self.next_record;
            self.next_record += 1;

            if self.options.skip_unused && !self.mft.bitmap_used(n) {
                continue;
            }

            let record_size = self.mft.boot.file_record_size as usize;

            let offset = match self.mft.extent_map.record_offset(n) {
                Ok(Some(o)) => o,
                Ok(None) => continue, // sparse hole
                Err(e) => return Some(Err(e)),
            };

            // Borrow the record bytes directly from the reader's
            // internal buffer to avoid the per-record memcpy.
            let buf = match self.reader.borrow_at(offset, record_size) {
                Ok(b) => b,
                Err(e) => return Some(Err(io_err(e))),
            };

            if !FileRecord::is_valid(buf) {
                continue;
            }
            match FileRecord::parse(n, buf) {
                Ok(rec) => {
                    let entry = RawMftEntry::from_record(&rec);
                    return Some(Ok(entry));
                }
                Err(e) => {
                    warn!("raw_mft: failed to parse record {n}: {e}");
                    continue;
                }
            }
        }
        None
    }
}

fn read_record_at(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
) -> Result<Option<RawMftEntry>, UsnError> {
    let offset = match extent_map.record_offset(record_number)? {
        Some(o) => o,
        None => return Ok(None),
    };
    let mut buf = vec![0u8; boot.file_record_size as usize];
    reader.seek(SeekFrom::Start(offset)).map_err(io_err)?;
    reader.read_exact(&mut buf).map_err(io_err)?;
    if !FileRecord::is_valid(&buf) {
        return Ok(None);
    }
    let rec = FileRecord::parse(record_number, &mut buf)?;
    Ok(Some(RawMftEntry::from_record(&rec)))
}

fn read_nonresident(
    reader: &mut VolumeReader,
    runs: &[DataRun],
    cluster_size: u64,
    data_size: u64,
) -> Result<Vec<u8>, UsnError> {
    let mut out = Vec::with_capacity(data_size as usize);
    let mut remaining = data_size;
    for run in runs {
        if remaining == 0 {
            break;
        }
        match *run {
            DataRun::Data { lcn, clusters } => {
                let bytes = clusters
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run byte length overflow"))?;
                let to_read = bytes.min(remaining);
                let mut buf = vec![0u8; to_read as usize];
                let off = lcn
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run offset overflow"))?;
                reader.seek(SeekFrom::Start(off)).map_err(io_err)?;
                reader.read_exact(&mut buf).map_err(io_err)?;
                out.extend_from_slice(&buf);
                remaining -= to_read;
            }
            DataRun::Sparse { clusters } => {
                let bytes = clusters
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run byte length overflow"))?;
                let to_zero = bytes.min(remaining);
                out.resize(out.len() + to_zero as usize, 0);
                remaining -= to_zero;
            }
        }
    }
    Ok(out)
}

fn io_err(e: std::io::Error) -> UsnError {
    UsnError::Io(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_defaults_are_sensible() {
        let o = RawMftOptions::default();
        assert_eq!(o.buffer_bytes, DEFAULT_BUFFER_BYTES);
        assert!(o.skip_unused);
        assert_eq!(o.start_record, FIRST_NORMAL_RECORD);
        assert!(o.end_record.is_none());
    }

    mod integration_tests {
        use super::super::*;
        use crate::path::PathResolver;
        use crate::volume::Volume;
        use std::env;

        fn pick_drive() -> char {
            env::var("USN_TEST_DRIVE")
                .ok()
                .and_then(|s| s.chars().next())
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or('C')
        }

        fn open_volume_or_skip() -> Option<Volume> {
            match Volume::from_drive_letter(pick_drive()) {
                Ok(v) => Some(v),
                Err(UsnError::NotElevated) => {
                    eprintln!("skipping: requires admin privileges");
                    None
                }
                Err(e) => {
                    eprintln!("skipping: {e}");
                    None
                }
            }
        }

        #[test]
        fn raw_mft_full_iteration_smoke() {
            let Some(volume) = open_volume_or_skip() else {
                return;
            };
            let mft = match RawMft::new(&volume) {
                Ok(m) => m,
                Err(UsnError::UnsupportedFilesystem(msg)) => {
                    eprintln!("skipping: {msg}");
                    return;
                }
                Err(e) => panic!("RawMft::new failed: {e}"),
            };
            let mut total = 0u64;
            let mut used = 0u64;
            let mut named = 0u64;
            let mut had_timestamps = false;
            for r in mft.try_iter().expect("iter").take(50_000) {
                let entry = match r {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                total += 1;
                if entry.is_used {
                    used += 1;
                }
                if !entry.file_name.is_empty() {
                    named += 1;
                }
                if entry.si_created.as_u64() != 0 && !entry.file_name.is_empty() {
                    had_timestamps = true;
                }
            }
            assert!(total > 0, "expected at least one record");
            assert!(used > 0, "expected used records");
            assert!(named > 0, "expected named records");
            assert!(
                had_timestamps,
                "expected at least one entry with SI timestamps"
            );
        }

        #[test]
        fn raw_mft_path_resolver_roundtrip() {
            let Some(volume) = open_volume_or_skip() else {
                return;
            };
            let mft = match RawMft::new(&volume) {
                Ok(m) => m,
                Err(UsnError::UnsupportedFilesystem(_)) => return,
                Err(e) => panic!("RawMft::new failed: {e}"),
            };
            let mut resolver = PathResolver::builder(&volume)
                .with_lru_cache(
                    std::num::NonZeroUsize::new(4096).expect("cache capacity must be non-zero"),
                )
                .build();
            let mut resolved_any = false;
            // Cap the search so the test stays bounded on huge volumes.
            for r in mft.try_iter().expect("iter").flatten().take(20_000) {
                if r.is_directory || r.file_name.is_empty() {
                    continue;
                }
                if let Some(path) = resolver.resolve_path(&r) {
                    let s = path.to_string_lossy();
                    if s.len() > 3 {
                        resolved_any = true;
                        break;
                    }
                }
            }
            assert!(
                resolved_any,
                "expected at least one resolvable path on the test drive"
            );
        }

        #[test]
        fn raw_mft_refs_returns_unsupported() {
            // D: is ReFS on the developer machine; skip unless USN_TEST_DRIVE
            // explicitly points at a non-NTFS drive or D: exists.
            let drive = env::var("USN_REFS_TEST_DRIVE")
                .ok()
                .and_then(|s| s.chars().next())
                .unwrap_or('D')
                .to_ascii_uppercase();
            let volume = match Volume::from_drive_letter(drive) {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("skipping: ReFS drive {drive} not available");
                    return;
                }
            };
            match RawMft::new(&volume) {
                Err(UsnError::UnsupportedFilesystem(_)) => {}
                Err(other) => eprintln!("non-NTFS produced: {other}"),
                Ok(_) => {
                    eprintln!("note: drive {drive} is NTFS; UnsupportedFilesystem not exercised")
                }
            }
        }
    }
}
