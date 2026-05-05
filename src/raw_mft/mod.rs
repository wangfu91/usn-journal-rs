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
mod batch;
mod boot;
mod data_run;
mod entry;
mod extent;
mod fixup;
mod io;
mod options;
mod record;
mod work_plan;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, warn};

use crate::{
    errors::UsnError,
    raw_mft::{
        attribute::{NtfsAttributeType, for_each_attr_list_entry, for_each_attribute},
        boot::BootSector,
        data_run::{DataRun, decode_runs},
        entry::{AttributeListInfo, EntryBuildOptions},
        extent::ExtentMap,
        io::VolumeReader,
        record::{FileRecord, MFT_RECORD_NUMBER},
    },
    volume::Volume,
};

pub use attribute::FileNameNamespace;
pub use batch::{RawMftBatchEntry, RawMftChunkBatch};
pub use data_run::{DataRun as DataRunInfo, DataRunSummary};
pub use entry::{AdsInfo, RawMftEntry, RawMftLink};
pub use options::{RawMftIterOptions, RawMftIterOptionsBuilder};
pub use work_plan::{RawMftWorkChunk, RawMftWorkPlanOptions};

/// Default I/O buffer size for raw `$MFT` iteration.
#[allow(clippy::useless_nonzero_new_unchecked)]
pub const DEFAULT_BUFFER_BYTES: NonZeroUsize = unsafe {
    // SAFETY: `256 * 1024` is a non-zero constant.
    NonZeroUsize::new_unchecked(256 * 1024)
};
const ATTR_READER_BUFFER_BYTES: usize = 64 * 1024;

/// Stage-by-stage timing and counters for raw `$MFT` parsing.
#[derive(Debug, Clone, Default)]
pub struct RawMftProfile {
    /// First record number included in the profile.
    pub start_record: u64,
    /// Exclusive end record number included in the profile.
    pub end_record: u64,
    /// Main sequential buffer size used by the profile run.
    pub buffer_bytes: usize,
    /// Total records considered in the configured range.
    pub records_examined: u64,
    /// Records skipped via the `$MFT` bitmap because they are not in use.
    pub records_skipped_unused: u64,
    /// Records whose logical offset resolves into a sparse hole.
    pub sparse_holes: u64,
    /// Records that failed the FILE signature / fixup validation.
    pub invalid_records: u64,
    /// Records whose parsed entry was skipped because they are extension records.
    pub extension_records_skipped: u64,
    /// Records that were successfully yielded by the current serial parser.
    pub records_yielded: u64,
    /// Records whose parse failed after validation.
    pub parse_errors: u64,
    /// Base records that triggered `$ATTRIBUTE_LIST` enrichment.
    pub attr_list_enrichments_attempted: u64,
    /// Enrichments that loaded at least one extension record.
    pub attr_list_enrichments_with_extension_loads: u64,
    /// Extension record references discovered in `$ATTRIBUTE_LIST` payloads.
    pub attr_list_extension_records_referenced: u64,
    /// Extension records successfully loaded during enrichment.
    pub attr_list_extension_records_loaded: u64,
    /// End-to-end wall time for the profile run.
    pub total_elapsed: Duration,
    /// Time spent checking the `$MFT` bitmap.
    pub bitmap_check_elapsed: Duration,
    /// Time spent resolving logical record numbers to volume offsets.
    pub record_offset_elapsed: Duration,
    /// Time spent borrowing record bytes from the buffered volume reader.
    pub borrow_elapsed: Duration,
    /// Time spent validating raw record buffers before parsing.
    pub validate_elapsed: Duration,
    /// Time spent in `FileRecord::parse`.
    pub parse_elapsed: Duration,
    /// Time spent converting parsed records into `RawMftEntry`.
    pub entry_build_elapsed: Duration,
    /// Time spent doing `$ATTRIBUTE_LIST` enrichment.
    pub attr_list_enrich_elapsed: Duration,
}

#[derive(Debug, Clone, Copy, Default)]
struct AttrListEnrichStats {
    extension_records_referenced: u64,
    extension_records_loaded: u64,
}

#[derive(Debug, Clone)]
enum ParallelVolumeSource {
    DriveLetter(char),
    MountPoint(PathBuf),
}

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
        let attr_reader = VolumeReader::with_buffer_bytes(
            self.volume.handle,
            self.boot.bytes_per_sector as u64,
            options.buffer_bytes.get().min(ATTR_READER_BUFFER_BYTES),
        )?;
        let total = self.record_count();
        let end = options.end_record.unwrap_or(total).min(total);
        Ok(RawMftIter {
            mft: self,
            reader,
            attr_reader,
            next_record: options.start_record,
            end,
            options,
        })
    }

    /// Run the current serial parser and return stage-by-stage timings.
    pub fn profile(&self) -> Result<RawMftProfile, UsnError> {
        self.profile_with_options(RawMftIterOptions::default())
    }

    /// Run the current serial parser with custom options and return stage timings.
    pub fn profile_with_options(
        &self,
        options: RawMftIterOptions,
    ) -> Result<RawMftProfile, UsnError> {
        let mut reader = VolumeReader::with_buffer_bytes(
            self.volume.handle,
            self.boot.bytes_per_sector as u64,
            options.buffer_bytes.get(),
        )?;
        let mut attr_reader = VolumeReader::with_buffer_bytes(
            self.volume.handle,
            self.boot.bytes_per_sector as u64,
            options.buffer_bytes.get().min(ATTR_READER_BUFFER_BYTES),
        )?;
        let end = options
            .end_record
            .unwrap_or(self.record_count())
            .min(self.record_count());
        let record_size = self.boot.file_record_size as usize;
        let build_options = EntryBuildOptions {
            collect_alternate_data_streams: options.collect_alternate_data_streams,
            collect_data_run_summary: options.collect_data_run_summary,
        };
        let mut profile = RawMftProfile {
            start_record: options.start_record,
            end_record: end,
            buffer_bytes: options.buffer_bytes.get(),
            ..RawMftProfile::default()
        };
        let total_start = Instant::now();

        let mut next_record = options.start_record;
        while next_record < end {
            let n = next_record;
            next_record += 1;
            profile.records_examined += 1;

            if options.skip_unused {
                let bitmap_start = Instant::now();
                let is_used = self.bitmap_used(n);
                profile.bitmap_check_elapsed += bitmap_start.elapsed();
                if !is_used {
                    profile.records_skipped_unused += 1;
                    continue;
                }
            }

            let record_offset_start = Instant::now();
            let offset = match self.extent_map.record_offset(n) {
                Ok(Some(offset)) => offset,
                Ok(None) => {
                    profile.record_offset_elapsed += record_offset_start.elapsed();
                    profile.sparse_holes += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            profile.record_offset_elapsed += record_offset_start.elapsed();

            let borrow_start = Instant::now();
            let buf = reader.borrow_at(offset, record_size).map_err(io_err)?;
            profile.borrow_elapsed += borrow_start.elapsed();

            let validate_start = Instant::now();
            let is_valid = FileRecord::is_valid(buf);
            profile.validate_elapsed += validate_start.elapsed();
            if !is_valid {
                profile.invalid_records += 1;
                continue;
            }

            let parse_start = Instant::now();
            let rec = match FileRecord::parse(n, Some(offset), buf) {
                Ok(record) => record,
                Err(error) => {
                    profile.parse_elapsed += parse_start.elapsed();
                    warn!("raw_mft: failed to parse record {n}: {error}");
                    profile.parse_errors += 1;
                    continue;
                }
            };
            profile.parse_elapsed += parse_start.elapsed();

            let entry_build_start = Instant::now();
            let (mut entry, attr_list) = RawMftEntry::from_record_with_attr_list(&rec, build_options);
            profile.entry_build_elapsed += entry_build_start.elapsed();

            if options.skip_extension_records && entry.base_record_reference != 0 {
                profile.extension_records_skipped += 1;
                continue;
            }

            if let Some(attr_list) = attr_list
                && should_enrich_from_attr_list(&entry)
            {
                profile.attr_list_enrichments_attempted += 1;
                let enrich_start = Instant::now();
                let enrich_stats = enrich_from_attr_list(
                    &mut entry,
                    attr_list,
                    n,
                    &mut attr_reader,
                    &self.boot,
                    &self.extent_map,
                    build_options,
                );
                profile.attr_list_enrich_elapsed += enrich_start.elapsed();
                profile.attr_list_extension_records_referenced +=
                    enrich_stats.extension_records_referenced;
                profile.attr_list_extension_records_loaded += enrich_stats.extension_records_loaded;
                if enrich_stats.extension_records_loaded > 0 {
                    profile.attr_list_enrichments_with_extension_loads += 1;
                }
            }

            profile.records_yielded += 1;
        }

        profile.total_elapsed = total_start.elapsed();
        Ok(profile)
    }

    /// Read a single record by number. Returns `Ok(None)` when the
    /// record falls in a sparse hole or is unused (and `skip_unused` is
    /// implied here).
    pub fn get_record(&self, number: u64) -> Result<Option<RawMftEntry>, UsnError> {
        let mut reader = VolumeReader::new(self.volume.handle, self.boot.bytes_per_sector as u64)?;
        read_record_at(
            &mut reader,
            &self.boot,
            &self.extent_map,
            number,
            EntryBuildOptions::full(),
        )
    }

    /// Build deterministic logical work chunks for raw `$MFT` parsing.
    #[must_use]
    pub fn plan_work_chunks(&self) -> Vec<RawMftWorkChunk> {
        self.plan_work_chunks_with_options(RawMftWorkPlanOptions::default())
    }

    /// Build logical work chunks with custom planning options.
    #[must_use]
    pub fn plan_work_chunks_with_options(
        &self,
        options: RawMftWorkPlanOptions,
    ) -> Vec<RawMftWorkChunk> {
        let end_record = options
            .end_record
            .unwrap_or(self.record_count())
            .min(self.record_count());
        work_plan::build_work_chunks(
            options.start_record,
            end_record,
            options.max_records_per_chunk,
            options.skip_unused,
            |record_number| self.bitmap_used(record_number),
        )
    }

    /// Parse one logical work chunk into lean batch entries.
    pub fn read_chunk_with_options(
        &self,
        chunk: RawMftWorkChunk,
        mut options: RawMftIterOptions,
    ) -> Result<Vec<RawMftBatchEntry>, UsnError> {
        options.start_record = chunk.start_record;
        options.end_record = Some(chunk.end_record);
        self.try_iter_with_options(options)?
            .map(|result| result.map(RawMftBatchEntry::from))
            .collect()
    }

    /// Parse logical work chunks in parallel using worker-local readers.
    pub fn read_chunks_parallel(
        &self,
        chunks: Vec<RawMftWorkChunk>,
    ) -> Result<Vec<RawMftChunkBatch>, UsnError> {
        let worker_count = thread::available_parallelism().map_err(|error| {
            UsnError::Io(std::io::Error::other(format!(
                "failed to query available parallelism: {error}"
            )))
        })?;
        self.read_chunks_parallel_with_options(chunks, RawMftIterOptions::default(), worker_count)
    }

    /// Parse logical work chunks in parallel, transform them on worker threads, and visit results
    /// in deterministic chunk order.
    pub fn for_each_mapped_chunk_parallel_with_options<F, T, V>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftIterOptions,
        worker_count: NonZeroUsize,
        map_chunk: F,
        mut visit: V,
    ) -> Result<(), UsnError>
    where
        F: Fn(RawMftChunkBatch) -> Result<T, UsnError> + Sync,
        T: Send,
        V: FnMut(T) -> Result<(), UsnError>,
    {
        if chunks.is_empty() {
            return Ok(());
        }

        let worker_count = worker_count.get().min(chunks.len()).max(1);
        if worker_count == 1 {
            for chunk in chunks {
                let entries = self.read_chunk_with_options(chunk, options.clone())?;
                let mapped = map_chunk(RawMftChunkBatch { chunk, entries })?;
                visit(mapped)?;
            }
            return Ok(());
        }

        let Some(source) = parallel_volume_source(self.volume) else {
            return Err(UsnError::Io(std::io::Error::other(
                "raw_mft parallel chunk parsing requires a reusable volume source",
            )));
        };
        let next_index = AtomicUsize::new(0);
        let chunk_count = chunks.len();
        let chunks = chunks.into_boxed_slice();
        let boot = self.boot.clone();
        let extent_map = self.extent_map.clone();
        let bitmap = self.bitmap.clone();

        thread::scope(|scope| -> Result<(), UsnError> {
            let (tx, rx) = mpsc::channel::<Result<(usize, T), UsnError>>();
            let mut handles = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                let next_index = &next_index;
                let chunks = &chunks;
                let tx = tx.clone();
                let options = options.clone();
                let source = source.clone();
                let boot = boot.clone();
                let extent_map = extent_map.clone();
                let bitmap = bitmap.clone();
                let map_chunk = &map_chunk;
                handles.push(scope.spawn(move || {
                    let volume = match open_parallel_volume(&source) {
                        Ok(volume) => volume,
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            return;
                        }
                    };
                    let worker_mft = RawMft {
                        volume: &volume,
                        boot,
                        extent_map,
                        bitmap,
                    };

                    loop {
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        if index >= chunks.len() {
                            break;
                        }
                        let chunk = chunks[index];
                        let mapped = worker_mft
                            .read_chunk_with_options(chunk, options.clone())
                            .and_then(|entries| map_chunk(RawMftChunkBatch { chunk, entries }))
                            .map(|mapped| (index, mapped));
                        if tx.send(mapped).is_err() {
                            break;
                        }
                    }
                }));
            }
            drop(tx);

            let mut next_expected = 0usize;
            let mut pending = BTreeMap::new();
            while next_expected < chunk_count {
                match rx.recv() {
                    Ok(Ok((index, mapped))) => {
                        if index == next_expected {
                            visit(mapped)?;
                            next_expected += 1;
                            while let Some(mapped) = pending.remove(&next_expected) {
                                visit(mapped)?;
                                next_expected += 1;
                            }
                        } else {
                            pending.insert(index, mapped);
                        }
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(_) => {
                        return Err(UsnError::Io(std::io::Error::other(
                            "raw_mft parallel chunk channel closed unexpectedly",
                        )));
                    }
                }
            }

            for handle in handles {
                if handle.join().is_err() {
                    return Err(UsnError::Io(std::io::Error::other(
                        "raw_mft parallel worker panicked",
                    )));
                }
            }
            Ok(())
        })
    }

    /// Parse logical work chunks in parallel and visit batches in deterministic order.
    pub fn for_each_chunk_parallel_with_options<F>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftIterOptions,
        worker_count: NonZeroUsize,
        visit: F,
    ) -> Result<(), UsnError>
    where
        F: FnMut(RawMftChunkBatch) -> Result<(), UsnError>,
    {
        self.for_each_mapped_chunk_parallel_with_options(
            chunks,
            options,
            worker_count,
            Ok::<_, UsnError>,
            visit,
        )
    }

    /// Parse logical work chunks in parallel using worker-local readers and custom options.
    pub fn read_chunks_parallel_with_options(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftIterOptions,
        worker_count: NonZeroUsize,
    ) -> Result<Vec<RawMftChunkBatch>, UsnError> {
        let mut ordered_batches = Vec::with_capacity(chunks.len());
        self.for_each_chunk_parallel_with_options(chunks, options, worker_count, |batch| {
            ordered_batches.push(batch);
            Ok(())
        })?;
        Ok(ordered_batches)
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
    /// Separate reader for random extension-record lookups so attr-list
    /// fixups do not mutate the iterator's sequential buffer window.
    attr_reader: VolumeReader,
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
                    let build_options = EntryBuildOptions {
                        collect_alternate_data_streams: self.options.collect_alternate_data_streams,
                        collect_data_run_summary: self.options.collect_data_run_summary,
                    };
                    let (mut entry, attr_list) =
                        RawMftEntry::from_record_with_attr_list(&rec, build_options);
                    if self.options.skip_extension_records && entry.base_record_reference != 0 {
                        continue;
                    }
                    // `rec` is last used above; NLL ends the borrow on
                    // the reader's internal buffer here, so self.reader
                    // is free for the extension-record reads below.
                    if let Some(al) = attr_list
                        && should_enrich_from_attr_list(&entry)
                    {
                        let _ = enrich_from_attr_list(
                            &mut entry,
                            al,
                            n,
                            &mut self.attr_reader,
                            &self.mft.boot,
                            &self.mft.extent_map,
                            build_options,
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
    build_options: EntryBuildOptions,
) -> Result<Option<(RawMftEntry, Option<AttributeListInfo>)>, UsnError> {
    let offset = match extent_map.record_offset(record_number)? {
        Some(o) => o,
        None => return Ok(None),
    };
    let buf = reader
        .borrow_at(offset, boot.file_record_size as usize)
        .map_err(io_err)?;
    if !FileRecord::is_valid(buf) {
        return Ok(None);
    }
    let rec = FileRecord::parse(record_number, Some(offset), buf)?;
    Ok(Some(RawMftEntry::from_record_with_attr_list(
        &rec,
        build_options,
    )))
}

/// Read a record and perform one level of `$ATTRIBUTE_LIST` enrichment.
fn read_record_at(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
    build_options: EntryBuildOptions,
) -> Result<Option<RawMftEntry>, UsnError> {
    let (mut entry, attr_list) =
        match read_record_raw(reader, boot, extent_map, record_number, build_options)? {
            Some(t) => t,
            None => return Ok(None),
        };
    if let Some(al) = attr_list
        && should_enrich_from_attr_list(&entry)
    {
        let _ = enrich_from_attr_list(
            &mut entry,
            al,
            record_number,
            reader,
            boot,
            extent_map,
            build_options,
        );
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
    build_options: EntryBuildOptions,
) -> AttrListEnrichStats {
    let mut stats = AttrListEnrichStats::default();
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
                    return stats;
                }
            };
            match read_nonresident(reader, &runs, boot.cluster_size, data_size) {
                Ok(b) => b,
                Err(e) => {
                    warn!(
                        "raw_mft: record {base_record_number}: \
                         failed to read non-resident $ATTRIBUTE_LIST: {e}"
                    );
                    return stats;
                }
            }
        }
    };

    // 2. Collect unique extension record numbers that hold data we may need to
    //    merge back into the base record.
    let mut ext_records: Vec<u64> = Vec::new();
    for_each_attr_list_entry(&data, |type_id, file_ref| {
        if matches!(
            type_id,
            x if x == NtfsAttributeType::FileName as u32
                || x == NtfsAttributeType::Data as u32
                || x == NtfsAttributeType::ReparsePoint as u32
        ) {
            let rec_num = file_ref & 0x0000_FFFF_FFFF_FFFF;
            if rec_num != base_record_number && !ext_records.contains(&rec_num) {
                ext_records.push(rec_num);
            }
        }
    });
    stats.extension_records_referenced = ext_records.len() as u64;

    // 3. For each extension record, look for a $FILE_NAME with a better
    //    namespace score than what the base record already carries.
    let mut best_score = if entry.file_name.is_empty() {
        -1
    } else {
        entry.namespace.score()
    };
    for ext_num in ext_records {
        // Use read_record_raw (no recursive enrichment) to avoid unbounded
        // depth in pathological MFTs.
        let ext_entry = match read_record_raw(reader, boot, extent_map, ext_num, build_options) {
            Ok(Some((e, _))) => {
                stats.extension_records_loaded += 1;
                e
            }
            Ok(None) => continue,
            Err(e) => {
                debug!(
                    "raw_mft: record {base_record_number}: \
                     failed to load extension record {ext_num}: {e}"
                );
                continue;
            }
        };
        let score = ext_entry.namespace.score();
        let ext_links: Vec<RawMftLink> =
            if ext_entry.links.is_empty() && !ext_entry.file_name.is_empty() {
                vec![RawMftLink {
                    parent_reference: ext_entry.parent_reference,
                    namespace: ext_entry.namespace,
                    file_name: ext_entry.file_name.clone(),
                }]
            } else {
                ext_entry.links.to_vec()
            };
        if !ext_links.is_empty() {
            let mut links = entry.links.to_vec();
            if links.is_empty() && !entry.file_name.is_empty() {
                links.push(RawMftLink {
                    parent_reference: entry.parent_reference,
                    namespace: entry.namespace,
                    file_name: entry.file_name.clone(),
                });
            }
            links.extend(ext_links);
            entry.links = links.into_boxed_slice();
        }
        merge_extension_data(entry, &ext_entry);
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
    stats
}

/// Merge relevant metadata from an extension record into the base record.
fn merge_extension_data(entry: &mut RawMftEntry, ext_entry: &RawMftEntry) {
    if ext_entry.real_size > entry.real_size || ext_entry.allocated_size > entry.allocated_size {
        entry.real_size = ext_entry.real_size;
        entry.allocated_size = ext_entry.allocated_size;
        entry.has_unnamed_data = ext_entry.has_unnamed_data;
        entry.is_resident = ext_entry.is_resident;
        entry.data_run_summary = ext_entry.data_run_summary.clone();
    }
    entry.is_sparse |= ext_entry.is_sparse;
    entry.is_compressed |= ext_entry.is_compressed;
    entry.is_encrypted |= ext_entry.is_encrypted;
    entry.is_reparse_point |= ext_entry.is_reparse_point;
    if entry.reparse_tag.is_none() {
        entry.reparse_tag = ext_entry.reparse_tag;
    }
    if !ext_entry.alternate_data_streams.is_empty() {
        let mut ads = entry.alternate_data_streams.to_vec();
        ads.extend_from_slice(&ext_entry.alternate_data_streams);
        entry.alternate_data_streams = ads.into_boxed_slice();
    }
}

/// Heuristic to decide whether to enrich a base record from its `$ATTRIBUTE_LIST` or not.
fn should_enrich_from_attr_list(entry: &RawMftEntry) -> bool {
    entry.file_name.is_empty()
        || !matches!(
            entry.namespace,
            FileNameNamespace::Win32 | FileNameNamespace::Win32AndDos
        )
        || entry.hard_link_count > 1
        || (!entry.is_directory && !entry.has_unnamed_data)
        || (entry.is_reparse_point && entry.reparse_tag.is_none())
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

fn parallel_volume_source(volume: &Volume) -> Option<ParallelVolumeSource> {
    volume
        .drive_letter()
        .map(ParallelVolumeSource::DriveLetter)
        .or_else(|| {
            volume
                .mount_point()
                .map(|path| ParallelVolumeSource::MountPoint(path.to_path_buf()))
        })
}

fn open_parallel_volume(source: &ParallelVolumeSource) -> Result<Volume, UsnError> {
    match source {
        ParallelVolumeSource::DriveLetter(drive_letter) => Volume::from_drive_letter(*drive_letter),
        ParallelVolumeSource::MountPoint(path) => Volume::from_mount_point(path),
    }
}
