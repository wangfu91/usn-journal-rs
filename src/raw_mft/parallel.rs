//! Public logical work-chunk APIs layered on top of the raw-MFT parser.

use std::{num::NonZeroUsize, thread};

use crate::{
    errors::UsnError,
    raw_mft::{
        attr_list::{enrich_batch_from_attr_list, should_enrich_batch_from_attr_list},
        batch::{RawMftBatchEntry, RawMftBatchScratch, RawMftChunkBatch},
        io::VolumeReader,
        options::RawMftIterOptions,
        serial_driver::{SerialParseState, next_record_output_with_hooks},
        work_plan::{self, RawMftWorkChunk, RawMftWorkPlanOptions},
    },
};

use super::{RawMft, parallel_executor};

impl<'a> RawMft<'a> {
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
        options: RawMftIterOptions,
    ) -> Result<Vec<RawMftBatchEntry>, UsnError> {
        let (mut reader, mut attr_reader) = self.buffered_readers_for_options(&options)?;
        self.read_chunk_with_reused_readers(chunk, &options, &mut reader, &mut attr_reader)
    }

    /// Parse one logical work chunk and fold lean batch entries into a caller-owned accumulator.
    pub fn fold_chunk_with_options<T, Init, Fold>(
        &self,
        chunk: RawMftWorkChunk,
        options: RawMftIterOptions,
        init: &Init,
        fold_entry: &Fold,
    ) -> Result<T, UsnError>
    where
        Init: Fn(RawMftWorkChunk) -> T,
        Fold: Fn(&mut T, RawMftBatchEntry) -> Result<(), UsnError>,
    {
        let (mut reader, mut attr_reader) = self.buffered_readers_for_options(&options)?;
        self.fold_chunk_with_reused_readers(
            chunk,
            &options,
            init,
            fold_entry,
            &mut reader,
            &mut attr_reader,
        )
    }

    /// Visit each batch entry in a record-number range using caller-supplied
    /// readers so chunk workers can reuse their buffer state.
    fn for_each_batch_entry_in_range_with_readers<F>(
        &self,
        start_record: u64,
        end_record: u64,
        options: &RawMftIterOptions,
        reader: &mut VolumeReader,
        attr_reader: &mut VolumeReader,
        mut visit: F,
    ) -> Result<(), UsnError>
    where
        F: FnMut(RawMftBatchEntry) -> Result<(), UsnError>,
    {
        let mut scan = SerialParseState::for_range(self, options, start_record, end_record);
        let mut hooks = ();

        while let Some(entry) = next_record_output_with_hooks(
            self,
            &mut scan,
            reader,
            &mut hooks,
            |record| {
                let record_number = record.number;

                let (mut entry, attr_list) = RawMftBatchScratch::from_record_with_attr_list(
                    record,
                    options.collect_dos_file_name_links,
                );

                if let Some(attr_list) = attr_list
                    && should_enrich_batch_from_attr_list(&entry)
                {
                    let _ = enrich_batch_from_attr_list(
                        &mut entry,
                        attr_list,
                        record_number,
                        attr_reader,
                        &self.boot,
                        self.extent_map.as_ref(),
                        options.collect_dos_file_name_links,
                    );
                }

                Ok(entry.into_entry())
            },
        )? {
            visit(entry)?;
        }

        Ok(())
    }

    /// Parse one chunk into batch entries using caller-supplied readers.
    fn read_chunk_with_reused_readers(
        &self,
        chunk: RawMftWorkChunk,
        options: &RawMftIterOptions,
        reader: &mut VolumeReader,
        attr_reader: &mut VolumeReader,
    ) -> Result<Vec<RawMftBatchEntry>, UsnError> {
        let mut entries = Vec::with_capacity(chunk.record_len().min(usize::MAX as u64) as usize);
        self.for_each_batch_entry_in_range_with_readers(
            chunk.start_record,
            chunk.end_record,
            options,
            reader,
            attr_reader,
            |entry| {
                entries.push(entry);
                Ok(())
            },
        )?;
        Ok(entries)
    }

    /// Fold one chunk into a caller-owned accumulator using caller-supplied readers.
    fn fold_chunk_with_reused_readers<T, Init, Fold>(
        &self,
        chunk: RawMftWorkChunk,
        options: &RawMftIterOptions,
        init: &Init,
        fold_entry: &Fold,
        reader: &mut VolumeReader,
        attr_reader: &mut VolumeReader,
    ) -> Result<T, UsnError>
    where
        Init: Fn(RawMftWorkChunk) -> T,
        Fold: Fn(&mut T, RawMftBatchEntry) -> Result<(), UsnError>,
    {
        let mut acc = init(chunk);
        self.for_each_batch_entry_in_range_with_readers(
            chunk.start_record,
            chunk.end_record,
            options,
            reader,
            attr_reader,
            |entry| fold_entry(&mut acc, entry),
        )?;
        Ok(acc)
    }

    /// Parse logical work chunks in parallel using worker-local readers.
    pub fn read_chunks_parallel(
        &self,
        chunks: Vec<RawMftWorkChunk>,
    ) -> Result<Vec<RawMftChunkBatch>, UsnError> {
        let worker_count =
            thread::available_parallelism().map_err(parallel_executor::available_parallelism_error)?;
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
        visit: V,
    ) -> Result<(), UsnError>
    where
        F: Fn(RawMftChunkBatch) -> Result<T, UsnError> + Sync,
        T: Send,
        V: FnMut(T) -> Result<(), UsnError>,
    {
        parallel_executor::run_parallel_chunks_in_order(
            self,
            chunks,
            options,
            worker_count,
            move |mft, chunk, options, reader, attr_reader| {
                mft.read_chunk_with_reused_readers(chunk, options, reader, attr_reader)
                    .and_then(|entries| map_chunk(RawMftChunkBatch { chunk, entries }))
            },
            visit,
        )
    }

    /// Parse logical work chunks in parallel and fold lean batch entries on worker threads.
    pub fn for_each_folded_chunk_parallel_with_options<Init, Fold, T, V>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftIterOptions,
        worker_count: NonZeroUsize,
        init: Init,
        fold_entry: Fold,
        visit: V,
    ) -> Result<(), UsnError>
    where
        Init: Fn(RawMftWorkChunk) -> T + Sync,
        Fold: Fn(&mut T, RawMftBatchEntry) -> Result<(), UsnError> + Sync,
        T: Send,
        V: FnMut(T) -> Result<(), UsnError>,
    {
        parallel_executor::run_parallel_chunks_in_order(
            self,
            chunks,
            options,
            worker_count,
            move |mft, chunk, options, reader, attr_reader| {
                mft.fold_chunk_with_reused_readers(
                    chunk,
                    options,
                    &init,
                    &fold_entry,
                    reader,
                    attr_reader,
                )
            },
            visit,
        )
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
}
