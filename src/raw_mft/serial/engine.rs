//! Shared serial scan engine for the raw-MFT iterator and serial chunk
//! walkers.

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        RawMft,
        io::VolumeReader,
        layout::{extent::ExtentLookupCursor, record::FileRecord},
        options::RawMftScanOptions,
        reader::io_err,
    },
};

/// Mutable state for one serial walk over logical raw-MFT record numbers.
pub(in crate::raw_mft) struct SerialParseState {
    next_record: u64,
    end_record: u64,
    offset_cursor: ExtentLookupCursor,
    record_size: usize,
    skip_unused: bool,
    skip_extension_records: bool,
}

impl SerialParseState {
    /// Create scan state that follows the logical range encoded in iterator options.
    pub(in crate::raw_mft) fn from_options(mft: &RawMft<'_>, options: &RawMftScanOptions) -> Self {
        let total = mft.record_count();
        let end_record = options.range.end_record.unwrap_or(total).min(total);
        Self::for_range(mft, options, options.range.start_record, end_record)
    }

    /// Create scan state for an explicit logical record range.
    pub(in crate::raw_mft) fn for_range(
        mft: &RawMft<'_>,
        options: &RawMftScanOptions,
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
pub(in crate::raw_mft) fn next_record_output<F, T>(
    mft: &RawMft<'_>,
    state: &mut SerialParseState,
    reader: &mut VolumeReader,
    mut on_record: F,
) -> Result<Option<T>, UsnError>
where
    F: FnMut(&FileRecord<'_>) -> Result<T, UsnError>,
{
    while state.next_record < state.end_record {
        let record_number = state.next_record;
        state.next_record += 1;

        if state.skip_unused && !mft.bitmap_used(record_number) {
            continue;
        }

        let offset_result = mft
            .extent_map
            .record_offset_with_cursor(record_number, &mut state.offset_cursor);
        let offset = match offset_result {
            Ok(Some(offset)) => offset,
            Ok(None) => continue,
            Err(error) => return Err(error),
        };

        let buf = reader
            .borrow_at(offset, state.record_size)
            .map_err(io_err)?;

        let is_valid = FileRecord::is_valid(buf);
        if !is_valid {
            continue;
        }

        let parse_result = FileRecord::parse(record_number, Some(offset), buf);
        let record = match parse_result {
            Ok(record) => record,
            Err(error) => {
                warn!("raw_mft: failed to parse record {record_number}: {error}");
                continue;
            }
        };

        if state.skip_extension_records && record.base_reference() != 0 {
            continue;
        }

        return on_record(&record).map(Some);
    }

    Ok(None)
}

