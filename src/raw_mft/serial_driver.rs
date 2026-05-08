//! Shared serial scan driver for the raw-MFT iterator, profiler, and
//! serial chunk walkers.

use std::time::Duration;

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        extent::ExtentLookupCursor,
        io::VolumeReader,
        options::RawMftIterOptions,
        reader::io_err,
        record::FileRecord,
    },
};

use super::RawMft;

/// Pre-entry-build stages shared by the serial raw-MFT scan paths.
#[derive(Debug, Clone, Copy)]
pub(super) enum SerialScanStage {
    /// `$MFT::$BITMAP` in-use check.
    BitmapCheck,
    /// Logical-record to byte-offset resolution.
    RecordOffset,
    /// Buffered read window lookup.
    Borrow,
    /// Cheap FILE-record validity check.
    Validate,
    /// Full `FileRecord::parse` call.
    Parse,
}

/// Hook points used by the shared serial scan driver.
pub(super) trait SerialScanHooks {
    /// Per-stage token used by the hook implementation.
    type StageToken;

    /// Start observing one scan stage.
    fn stage_start(&mut self, _stage: SerialScanStage) -> Self::StageToken;

    /// Finish observing one scan stage.
    fn stage_finish(&mut self, _stage: SerialScanStage, _token: Self::StageToken);

    /// Observe that one logical record number is being examined.
    fn on_record_examined(&mut self, _record_number: u64) {}

    /// Observe that one record was dropped by the `$MFT::$BITMAP` check.
    fn on_skipped_unused(&mut self, _record_number: u64) {}

    /// Observe that one record resolved into a sparse hole.
    fn on_sparse_hole(&mut self, _record_number: u64) {}

    /// Observe that one raw buffer failed FILE-record validation.
    fn on_invalid_record(&mut self, _record_number: u64) {}

    /// Observe that one record failed `FileRecord::parse`.
    fn on_parse_error(&mut self, _record_number: u64, _error: &UsnError) {}

    /// Observe that one parsed record was skipped because it is an extension
    /// record.
    fn on_extension_record_skipped(&mut self, _record_number: u64) {}
}

impl SerialScanHooks for () {
    type StageToken = ();

    fn stage_start(&mut self, _stage: SerialScanStage) -> Self::StageToken {}

    fn stage_finish(&mut self, _stage: SerialScanStage, _token: Self::StageToken) {}
}

/// Mutable state for one serial walk over logical raw-MFT record numbers.
pub(super) struct SerialParseState {
    next_record: u64,
    end_record: u64,
    offset_cursor: ExtentLookupCursor,
    record_size: usize,
    skip_unused: bool,
    skip_extension_records: bool,
}

impl SerialParseState {
    /// Create scan state that follows the logical range encoded in iterator options.
    pub(super) fn from_options(mft: &RawMft<'_>, options: &RawMftIterOptions) -> Self {
        let total = mft.record_count();
        let end_record = options.end_record.unwrap_or(total).min(total);
        Self::for_range(mft, options, options.start_record, end_record)
    }

    /// Create scan state for an explicit logical record range.
    pub(super) fn for_range(
        mft: &RawMft<'_>,
        options: &RawMftIterOptions,
        start_record: u64,
        end_record: u64,
    ) -> Self {
        let total = mft.record_count();
        let end_record = end_record.min(total);
        Self {
            next_record: start_record.min(end_record),
            end_record,
            offset_cursor: ExtentLookupCursor::default(),
            record_size: mft.boot.file_record_size as usize,
            skip_unused: options.skip_unused,
            skip_extension_records: options.skip_extension_records,
        }
    }
}

/// Yield the next owned output produced from a parsed base FILE record.
pub(super) fn next_record_output_with_hooks<H, F, T>(
    mft: &RawMft<'_>,
    state: &mut SerialParseState,
    reader: &mut VolumeReader,
    hooks: &mut H,
    mut on_record: F,
) -> Result<Option<T>, UsnError>
where
    H: SerialScanHooks,
    F: FnMut(&FileRecord<'_>) -> Result<T, UsnError>,
{
    while state.next_record < state.end_record {
        let record_number = state.next_record;
        state.next_record += 1;
        hooks.on_record_examined(record_number);

        if state.skip_unused {
            let token = hooks.stage_start(SerialScanStage::BitmapCheck);
            let is_used = mft.bitmap_used(record_number);
            hooks.stage_finish(SerialScanStage::BitmapCheck, token);
            if !is_used {
                hooks.on_skipped_unused(record_number);
                continue;
            }
        }

        let token = hooks.stage_start(SerialScanStage::RecordOffset);
        let offset_result = mft
            .extent_map
            .record_offset_with_cursor(record_number, &mut state.offset_cursor);
        hooks.stage_finish(SerialScanStage::RecordOffset, token);
        let offset = match offset_result {
            Ok(Some(offset)) => offset,
            Ok(None) => {
                hooks.on_sparse_hole(record_number);
                continue;
            }
            Err(error) => return Err(error),
        };

        let token = hooks.stage_start(SerialScanStage::Borrow);
        let buf = reader.borrow_at(offset, state.record_size).map_err(io_err)?;
        hooks.stage_finish(SerialScanStage::Borrow, token);

        let token = hooks.stage_start(SerialScanStage::Validate);
        let is_valid = FileRecord::is_valid(buf);
        hooks.stage_finish(SerialScanStage::Validate, token);
        if !is_valid {
            hooks.on_invalid_record(record_number);
            continue;
        }

        let token = hooks.stage_start(SerialScanStage::Parse);
        let parse_result = FileRecord::parse(record_number, Some(offset), buf);
        hooks.stage_finish(SerialScanStage::Parse, token);
        let record = match parse_result {
            Ok(record) => record,
            Err(error) => {
                warn!("raw_mft: failed to parse record {record_number}: {error}");
                hooks.on_parse_error(record_number, &error);
                continue;
            }
        };

        if state.skip_extension_records && record.base_reference() != 0 {
            hooks.on_extension_record_skipped(record_number);
            continue;
        }

        return on_record(&record).map(Some);
    }

    Ok(None)
}

/// Add elapsed time to the stage accumulator selected by the caller.
pub(super) fn accumulate_stage_elapsed(
    bitmap_check_elapsed: &mut Duration,
    record_offset_elapsed: &mut Duration,
    borrow_elapsed: &mut Duration,
    validate_elapsed: &mut Duration,
    parse_elapsed: &mut Duration,
    stage: SerialScanStage,
    elapsed: Duration,
) {
    match stage {
        SerialScanStage::BitmapCheck => *bitmap_check_elapsed += elapsed,
        SerialScanStage::RecordOffset => *record_offset_elapsed += elapsed,
        SerialScanStage::Borrow => *borrow_elapsed += elapsed,
        SerialScanStage::Validate => *validate_elapsed += elapsed,
        SerialScanStage::Parse => *parse_elapsed += elapsed,
    }
}