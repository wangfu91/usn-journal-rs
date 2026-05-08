//! Serial raw-MFT iteration over logical record numbers.

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        attr_list::{enrich_from_attr_list, should_enrich_from_attr_list},
        entry::{EntryBuildOptions, RawMftEntry},
        extent::ExtentLookupCursor,
        io::VolumeReader,
        options::RawMftIterOptions,
        reader::{entry_build_options, io_err},
        record::FileRecord,
    },
};

use super::RawMft;

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
    /// Cursor tracking the last extent segment used for sequential lookups.
    offset_cursor: ExtentLookupCursor,
    /// Fixed FILE record size for the active volume.
    record_size: usize,
    /// Active iteration options.
    options: RawMftIterOptions,
    /// Cached entry-build options derived from the active iterator options.
    build_options: EntryBuildOptions,
}

impl<'a> RawMft<'a> {
    /// Begin iteration with default options.
    pub fn try_iter(&self) -> Result<RawMftIter<'_>, UsnError> {
        self.try_iter_with_options(RawMftIterOptions::default())
    }

    /// Begin iteration with custom options.
    pub fn try_iter_with_options(
        &self,
        options: RawMftIterOptions,
    ) -> Result<RawMftIter<'_>, UsnError> {
        let (reader, attr_reader) = self.buffered_readers_for_options(&options)?;
        let total = self.record_count();
        let end = options.end_record.unwrap_or(total).min(total);
        let build_options = entry_build_options(&options);

        Ok(RawMftIter {
            mft: self,
            reader,
            attr_reader,
            next_record: options.start_record,
            end,
            offset_cursor: ExtentLookupCursor::default(),
            record_size: self.boot.file_record_size as usize,
            options,
            build_options,
        })
    }
}

impl<'a> Iterator for RawMftIter<'a> {
    type Item = Result<RawMftEntry, UsnError>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next_record < self.end {
            let record_number = self.next_record;
            self.next_record += 1;

            if self.options.skip_unused && !self.mft.bitmap_used(record_number) {
                continue;
            }

            let offset = match self
                .mft
                .extent_map
                .record_offset_with_cursor(record_number, &mut self.offset_cursor)
            {
                Ok(Some(offset)) => offset,
                Ok(None) => continue,
                Err(error) => return Some(Err(error)),
            };

            let buf = match self.reader.borrow_at(offset, self.record_size) {
                Ok(buf) => buf,
                Err(error) => return Some(Err(io_err(error))),
            };

            if !FileRecord::is_valid(buf) {
                continue;
            }

            match FileRecord::parse(record_number, Some(offset), buf) {
                Ok(record) => {
                    if self.options.skip_extension_records && record.base_reference() != 0 {
                        continue;
                    }

                    let (mut entry, attr_list) =
                        RawMftEntry::from_record_with_attr_list(&record, self.build_options);
                    if let Some(attr_list) = attr_list
                        && should_enrich_from_attr_list(&entry)
                    {
                        let _ = enrich_from_attr_list(
                            &mut entry,
                            attr_list,
                            record_number,
                            &mut self.attr_reader,
                            &self.mft.boot,
                            self.mft.extent_map.as_ref(),
                            self.build_options,
                        );
                    }
                    return Some(Ok(entry));
                }
                Err(error) => {
                    warn!("raw_mft: failed to parse record {record_number}: {error}");
                    continue;
                }
            }
        }

        None
    }
}