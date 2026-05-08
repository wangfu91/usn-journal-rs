//! Profiling helpers for the serial raw-MFT parsing pipeline.

use std::time::{Duration, Instant};

use crate::{
    errors::UsnError,
    raw_mft::{
        attr_list::{enrich_from_attr_list, should_enrich_from_attr_list},
        options::RawMftScanOptions,
        reader::entry_build_options,
        serial_driver::{
            SerialParseState, SerialScanHooks, SerialScanStage, accumulate_stage_elapsed,
            next_record_output_with_hooks,
        },
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

struct ProfileScanHooks<'a> {
    stats: &'a mut ProfileScanStats,
}

#[derive(Default)]
struct ProfileScanStats {
    records_examined: u64,
    records_skipped_unused: u64,
    sparse_holes: u64,
    invalid_records: u64,
    extension_records_skipped: u64,
    parse_errors: u64,
    bitmap_check_elapsed: Duration,
    record_offset_elapsed: Duration,
    borrow_elapsed: Duration,
    validate_elapsed: Duration,
    parse_elapsed: Duration,
}

impl SerialScanHooks for ProfileScanHooks<'_> {
    type StageToken = Instant;

    fn stage_start(&mut self, _stage: SerialScanStage) -> Self::StageToken {
        Instant::now()
    }

    fn stage_finish(&mut self, stage: SerialScanStage, token: Self::StageToken) {
        accumulate_stage_elapsed(
            &mut self.stats.bitmap_check_elapsed,
            &mut self.stats.record_offset_elapsed,
            &mut self.stats.borrow_elapsed,
            &mut self.stats.validate_elapsed,
            &mut self.stats.parse_elapsed,
            stage,
            token.elapsed(),
        );
    }

    fn on_record_examined(&mut self, _record_number: u64) {
        self.stats.records_examined += 1;
    }

    fn on_skipped_unused(&mut self, _record_number: u64) {
        self.stats.records_skipped_unused += 1;
    }

    fn on_sparse_hole(&mut self, _record_number: u64) {
        self.stats.sparse_holes += 1;
    }

    fn on_invalid_record(&mut self, _record_number: u64) {
        self.stats.invalid_records += 1;
    }

    fn on_parse_error(&mut self, _record_number: u64, _error: &UsnError) {
        self.stats.parse_errors += 1;
    }

    fn on_extension_record_skipped(&mut self, _record_number: u64) {
        self.stats.extension_records_skipped += 1;
    }
}

impl<'a> RawMft<'a> {
    /// Run the current serial parser and return stage-by-stage timings.
    pub fn profile(&self) -> Result<RawMftProfile, UsnError> {
        self.profile_with_options(RawMftScanOptions::default())
    }

    /// Run the current serial parser with custom options and return stage timings.
    pub fn profile_with_options(
        &self,
        options: RawMftScanOptions,
    ) -> Result<RawMftProfile, UsnError> {
        let (mut reader, mut attr_reader) = self.buffered_readers_for_options(&options)?;
        let end = options
            .range
            .end_record
            .unwrap_or(self.record_count())
            .min(self.record_count());
        let build_options = entry_build_options(&options);
        let mut scan = SerialParseState::from_options(self, &options);
        let mut profile = RawMftProfile {
            start_record: options.range.start_record,
            end_record: end,
            buffer_bytes: options.buffers.main.get(),
            ..RawMftProfile::default()
        };
        let mut scan_stats = ProfileScanStats::default();
        let total_start = Instant::now();

        loop {
            let next = {
                let mut hooks = ProfileScanHooks {
                    stats: &mut scan_stats,
                };
                next_record_output_with_hooks(self, &mut scan, &mut reader, &mut hooks, |record| {
                    let record_number = record.number;

                    let entry_build_start = Instant::now();
                    let (mut entry, attr_list) =
                        crate::raw_mft::entry::RawMftEntry::from_record_with_attr_list(
                            record,
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
                    Ok(())
                })?
            };

            if next.is_none() {
                break;
            }
        }

        profile.records_examined = scan_stats.records_examined;
        profile.records_skipped_unused = scan_stats.records_skipped_unused;
        profile.sparse_holes = scan_stats.sparse_holes;
        profile.invalid_records = scan_stats.invalid_records;
        profile.extension_records_skipped = scan_stats.extension_records_skipped;
        profile.parse_errors = scan_stats.parse_errors;
        profile.bitmap_check_elapsed = scan_stats.bitmap_check_elapsed;
        profile.record_offset_elapsed = scan_stats.record_offset_elapsed;
        profile.borrow_elapsed = scan_stats.borrow_elapsed;
        profile.validate_elapsed = scan_stats.validate_elapsed;
        profile.parse_elapsed = scan_stats.parse_elapsed;

        profile.total_elapsed = total_start.elapsed();
        Ok(profile)
    }
}
