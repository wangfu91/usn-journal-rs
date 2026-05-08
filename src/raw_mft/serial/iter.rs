//! Serial raw-MFT iteration over logical record numbers.

use crate::{
    errors::UsnError,
    raw_mft::{
        RawMft,
        attr_list::{enrich_from_attr_list, should_enrich_from_attr_list},
        entry_build::{EntryBuildOptions, RawMftEntry},
        io::VolumeReader,
        options::RawMftScanOptions,
        reader::entry_build_options,
    },
};

use super::engine::{SerialParseState, next_record_output_with_hooks};

/// Streaming iterator over MFT records.
pub struct RawMftIter<'a> {
    /// Parent raw-MFT reader.
    mft: &'a RawMft<'a>,
    /// Sector-aligned volume reader reused across iteration.
    reader: VolumeReader,
    /// Separate reader for random extension-record lookups so attr-list
    /// fixups do not mutate the iterator's sequential buffer window.
    attr_reader: VolumeReader,
    /// Shared serial scan state for the record walk.
    scan: SerialParseState,
    /// Cached entry-build options derived from the active iterator options.
    build_options: EntryBuildOptions,
}

impl<'a> RawMft<'a> {
    /// Begin iteration with default options.
    pub fn iter(&self) -> Result<RawMftIter<'_>, UsnError> {
        self.iter_with_options(RawMftScanOptions::default())
    }

    /// Begin iteration with custom options.
    pub fn iter_with_options(
        &self,
        options: RawMftScanOptions,
    ) -> Result<RawMftIter<'_>, UsnError> {
        let (reader, attr_reader) = self.buffered_readers_for_options(&options)?;
        let scan = SerialParseState::from_options(self, &options);
        let build_options = entry_build_options(&options);

        Ok(RawMftIter {
            mft: self,
            reader,
            attr_reader,
            scan,
            build_options,
        })
    }
}

impl<'a> Iterator for RawMftIter<'a> {
    type Item = Result<RawMftEntry, UsnError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut hooks = ();
        match next_record_output_with_hooks(
            self.mft,
            &mut self.scan,
            &mut self.reader,
            &mut hooks,
            |record| {
                let record_number = record.number;
                let (mut entry, attr_list) =
                    RawMftEntry::from_record_with_attr_list(record, self.build_options);
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
                Ok(entry)
            },
        ) {
            Ok(Some(entry)) => Some(Ok(entry)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}
