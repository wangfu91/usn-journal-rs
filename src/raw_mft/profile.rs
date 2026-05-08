//! Profiling helpers for the serial raw-MFT parsing pipeline.

use std::time::{Duration, Instant};

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        attr_list::{enrich_from_attr_list, should_enrich_from_attr_list},
        extent::ExtentLookupCursor,
        options::RawMftIterOptions,
        reader::{entry_build_options, io_err},
        record::FileRecord,
    },
};

use super::RawMft;

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

impl<'a> RawMft<'a> {
    /// Run the current serial parser and return stage-by-stage timings.
    pub fn profile(&self) -> Result<RawMftProfile, UsnError> {
        self.profile_with_options(RawMftIterOptions::default())
    }

    /// Run the current serial parser with custom options and return stage timings.
    pub fn profile_with_options(
        &self,
        options: RawMftIterOptions,
    ) -> Result<RawMftProfile, UsnError> {
        let (mut reader, mut attr_reader) = self.buffered_readers_for_options(&options)?;
        let end = options
            .end_record
            .unwrap_or(self.record_count())
            .min(self.record_count());
        let record_size = self.boot.file_record_size as usize;
        let build_options = entry_build_options(&options);
        let mut offset_cursor = ExtentLookupCursor::default();
        let mut profile = RawMftProfile {
            start_record: options.start_record,
            end_record: end,
            buffer_bytes: options.buffer_bytes.get(),
            ..RawMftProfile::default()
        };
        let total_start = Instant::now();

        let mut next_record = options.start_record;
        while next_record < end {
            let record_number = next_record;
            next_record += 1;
            profile.records_examined += 1;

            if options.skip_unused {
                let bitmap_start = Instant::now();
                let is_used = self.bitmap_used(record_number);
                profile.bitmap_check_elapsed += bitmap_start.elapsed();
                if !is_used {
                    profile.records_skipped_unused += 1;
                    continue;
                }
            }

            let offset_start = Instant::now();
            let offset = match self
                .extent_map
                .record_offset_with_cursor(record_number, &mut offset_cursor)
            {
                Ok(Some(offset)) => offset,
                Ok(None) => {
                    profile.record_offset_elapsed += offset_start.elapsed();
                    profile.sparse_holes += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            profile.record_offset_elapsed += offset_start.elapsed();

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
            let record = match FileRecord::parse(record_number, Some(offset), buf) {
                Ok(record) => record,
                Err(error) => {
                    profile.parse_elapsed += parse_start.elapsed();
                    warn!("raw_mft: failed to parse record {record_number}: {error}");
                    profile.parse_errors += 1;
                    continue;
                }
            };
            profile.parse_elapsed += parse_start.elapsed();

            if options.skip_extension_records && record.base_reference() != 0 {
                profile.extension_records_skipped += 1;
                continue;
            }

            let entry_build_start = Instant::now();
            let (mut entry, attr_list) =
                crate::raw_mft::entry::RawMftEntry::from_record_with_attr_list(
                    &record,
                    build_options,
                );
            profile.entry_build_elapsed += entry_build_start.elapsed();

            if let Some(attr_list) = attr_list
                && should_enrich_from_attr_list(&entry)
            {
                profile.attr_list_enrichments_attempted += 1;
                let enrich_start = Instant::now();
                let enrich_stats = enrich_from_attr_list(
                    &mut entry,
                    attr_list,
                    record_number,
                    &mut attr_reader,
                    &self.boot,
                    self.extent_map.as_ref(),
                    build_options,
                );
                profile.attr_list_enrich_elapsed += enrich_start.elapsed();
                profile.attr_list_extension_records_referenced +=
                    enrich_stats.extension_records_referenced;
                profile.attr_list_extension_records_loaded +=
                    enrich_stats.extension_records_loaded;
                if enrich_stats.extension_records_loaded > 0 {
                    profile.attr_list_enrichments_with_extension_loads += 1;
                }
            }

            profile.records_yielded += 1;
        }

        profile.total_elapsed = total_start.elapsed();
        Ok(profile)
    }
}