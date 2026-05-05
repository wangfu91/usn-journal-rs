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
mod options;
mod record;
#[cfg(test)]
mod tests;

use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroUsize;

use log::{debug, warn};

use crate::{
    errors::UsnError,
    raw_mft::{
        attribute::{NtfsAttributeType, for_each_attr_list_entry, for_each_attribute},
        boot::BootSector,
        data_run::{DataRun, decode_runs},
        entry::AttributeListInfo,
        extent::ExtentMap,
        io::VolumeReader,
        record::{FileRecord, MFT_RECORD_NUMBER},
    },
    volume::Volume,
};

pub use attribute::FileNameNamespace;
pub use data_run::{DataRun as DataRunInfo, DataRunSummary};
pub use entry::{AdsInfo, RawMftEntry};
pub use options::{RawMftIterOptions, RawMftIterOptionsBuilder};

/// Default I/O buffer size for raw `$MFT` iteration.
pub const DEFAULT_BUFFER_BYTES: NonZeroUsize = match NonZeroUsize::new(256 * 1024) {
    Some(v) => v,
    None => unreachable!(),
};

/// Raw `$MFT` reader bound to an open [`Volume`].
pub struct RawMft<'a> {
    /// Volume whose `$MFT` is being read.
    volume: &'a Volume,
    /// Parsed NTFS boot-sector geometry.
    boot: BootSector,
    /// Extent map for the `$MFT` data stream.
    extent_map: ExtentMap,
    /// Contents of the `$MFT` bitmap stream when available.
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
            let parsed =
                FileRecord::parse(MFT_RECORD_NUMBER, Some(boot.mft_byte_offset), &mut record0)?;

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
        self.try_iter_with_options(RawMftIterOptions::default())
    }

    /// Begin iteration with custom options.
    pub fn try_iter_with_options(
        &self,
        options: RawMftIterOptions,
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
    /// Parent raw-MFT reader.
    mft: &'a RawMft<'a>,
    /// Sector-aligned volume reader reused across iteration.
    reader: VolumeReader,
    /// Next record number to examine.
    next_record: u64,
    /// Exclusive end record number.
    end: u64,
    /// Active iteration options.
    options: RawMftIterOptions,
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
            match FileRecord::parse(n, Some(offset), buf) {
                Ok(rec) => {
                    let (mut entry, attr_list) = RawMftEntry::from_record_with_attr_list(&rec);
                    // `rec` is last used above; NLL ends the borrow on
                    // the reader's internal buffer here, so self.reader
                    // is free for the extension-record reads below.
                    if let Some(al) = attr_list {
                        enrich_from_attr_list(
                            &mut entry,
                            al,
                            n,
                            &mut self.reader,
                            &self.mft.boot,
                            &self.mft.extent_map,
                        );
                    }
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

/// Read a raw FILE record and return the parsed entry plus any `$ATTRIBUTE_LIST`.
fn read_record_raw(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
) -> Result<Option<(RawMftEntry, Option<AttributeListInfo>)>, UsnError> {
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
    let rec = FileRecord::parse(record_number, Some(offset), &mut buf)?;
    Ok(Some(RawMftEntry::from_record_with_attr_list(&rec)))
}

/// Read a record and perform one level of `$ATTRIBUTE_LIST` enrichment.
fn read_record_at(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
) -> Result<Option<RawMftEntry>, UsnError> {
    let (mut entry, attr_list) = match read_record_raw(reader, boot, extent_map, record_number)? {
        Some(t) => t,
        None => return Ok(None),
    };
    if let Some(al) = attr_list {
        enrich_from_attr_list(&mut entry, al, record_number, reader, boot, extent_map);
    }
    Ok(Some(entry))
}

/// Enrich a base-record entry by reading the extension records named in
/// its `$ATTRIBUTE_LIST` and adopting any `$FILE_NAME` attribute with a
/// higher namespace score (e.g. Win32 over Dos).
///
/// This fixes the case where a file with many hard links has its Win32
/// long name stored in an extension record while the base record only
/// holds the DOS 8.3 short name.
fn enrich_from_attr_list(
    entry: &mut RawMftEntry,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
) {
    // 1. Materialise the flat $ATTRIBUTE_LIST byte slice.
    let data: Vec<u8> = match attr_list {
        AttributeListInfo::Resident(bytes) => bytes,
        AttributeListInfo::NonResident {
            runs_data,
            data_size,
        } => {
            let runs = match decode_runs(&runs_data) {
                Ok((r, _)) => r,
                Err(e) => {
                    warn!(
                        "raw_mft: record {base_record_number}: \
                         failed to decode $ATTRIBUTE_LIST data runs: {e}"
                    );
                    return;
                }
            };
            match read_nonresident(reader, &runs, boot.cluster_size, data_size) {
                Ok(b) => b,
                Err(e) => {
                    warn!(
                        "raw_mft: record {base_record_number}: \
                         failed to read non-resident $ATTRIBUTE_LIST: {e}"
                    );
                    return;
                }
            }
        }
    };

    // 2. Collect unique extension record numbers that hold a $FILE_NAME attr.
    let mut ext_records: Vec<u64> = Vec::new();
    for_each_attr_list_entry(&data, |type_id, file_ref| {
        if type_id == NtfsAttributeType::FileName as u32 {
            let rec_num = file_ref & 0x0000_FFFF_FFFF_FFFF;
            if rec_num != base_record_number && !ext_records.contains(&rec_num) {
                ext_records.push(rec_num);
            }
        }
    });

    // 3. For each extension record, look for a $FILE_NAME with a better
    //    namespace score than what the base record already carries.
    let mut best_score = entry.namespace.score();
    for ext_num in ext_records {
        // Use read_record_raw (no recursive enrichment) to avoid unbounded
        // depth in pathological MFTs.
        let ext_entry = match read_record_raw(reader, boot, extent_map, ext_num) {
            Ok(Some((e, _))) => e,
            Ok(None) => continue,
            Err(e) => {
                warn!(
                    "raw_mft: record {base_record_number}: \
                     failed to load extension record {ext_num}: {e}"
                );
                continue;
            }
        };
        let score = ext_entry.namespace.score();
        if score > best_score {
            best_score = score;
            entry.namespace = ext_entry.namespace;
            entry.file_name = ext_entry.file_name;
            entry.parent_reference = ext_entry.parent_reference;
            entry.fn_created = ext_entry.fn_created;
            entry.fn_modified = ext_entry.fn_modified;
            entry.fn_mft_modified = ext_entry.fn_mft_modified;
            entry.fn_accessed = ext_entry.fn_accessed;
        }
    }
}

/// Materialize the bytes of a non-resident attribute from its decoded runs.
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
                let off = lcn
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run offset overflow"))?;
                let start = out.len();
                out.resize(start + to_read as usize, 0);
                reader.seek(SeekFrom::Start(off)).map_err(io_err)?;
                reader.read_exact(&mut out[start..]).map_err(io_err)?;
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

/// Convert a standard I/O error into the crate's error type.
fn io_err(e: std::io::Error) -> UsnError {
    UsnError::Io(e)
}
