//! Logical work-chunk planning and parallel raw-MFT execution helpers.

use std::{
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        attr_list::{enrich_batch_from_attr_list, should_enrich_batch_from_attr_list},
        batch::{RawMftBatchEntry, RawMftBatchScratch, RawMftChunkBatch},
        extent::ExtentLookupCursor,
        io::VolumeReader,
        options::RawMftIterOptions,
        reader::io_err,
        record::FileRecord,
        work_plan::{self, RawMftWorkChunk, RawMftWorkPlanOptions},
    },
    volume::Volume,
};

use super::RawMft;

/// Reopenable source information for worker-local volume handles.
#[derive(Debug, Clone)]
enum ParallelVolumeSource {
    DriveLetter(char),
    MountPoint(PathBuf),
}

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
        let end_record = end_record.min(self.record_count());
        let record_size = self.boot.file_record_size as usize;
        let mut next_record = start_record;
        let mut offset_cursor = ExtentLookupCursor::default();

        while next_record < end_record {
            let record_number = next_record;
            next_record += 1;

            if options.skip_unused && !self.bitmap_used(record_number) {
                continue;
            }

            let offset = match self
                .extent_map
                .record_offset_with_cursor(record_number, &mut offset_cursor)
            {
                Ok(Some(offset)) => offset,
                Ok(None) => continue,
                Err(error) => return Err(error),
            };

            let buf = reader.borrow_at(offset, record_size).map_err(io_err)?;
            if !FileRecord::is_valid(buf) {
                continue;
            }

            let record = match FileRecord::parse(record_number, Some(offset), buf) {
                Ok(record) => record,
                Err(error) => {
                    warn!("raw_mft: failed to parse record {record_number}: {error}");
                    continue;
                }
            };

            if options.skip_extension_records && record.base_reference() != 0 {
                continue;
            }

            let (mut entry, attr_list) = RawMftBatchScratch::from_record_with_attr_list(
                &record,
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

            visit(entry.into_entry())?;
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
        let worker_count = thread::available_parallelism().map_err(available_parallelism_error)?;
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
        self.run_parallel_chunks_in_order(chunks, options, worker_count, move |mft, chunk, options, reader, attr_reader| {
            mft.read_chunk_with_reused_readers(chunk, options, reader, attr_reader)
                .and_then(|entries| map_chunk(RawMftChunkBatch { chunk, entries }))
        }, visit)
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
        self.run_parallel_chunks_in_order(
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

    /// Run chunk work in parallel and visit results in original chunk order.
    fn run_parallel_chunks_in_order<T, Work, Visit>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftIterOptions,
        worker_count: NonZeroUsize,
        work_chunk: Work,
        mut visit: Visit,
    ) -> Result<(), UsnError>
    where
        for<'m> Work: Fn(
                &RawMft<'m>,
                RawMftWorkChunk,
                &RawMftIterOptions,
                &mut VolumeReader,
                &mut VolumeReader,
            ) -> Result<T, UsnError>
            + Sync,
        T: Send,
        Visit: FnMut(T) -> Result<(), UsnError>,
    {
        if chunks.is_empty() {
            return Ok(());
        }

        let worker_count = worker_count.get().min(chunks.len()).max(1);
        if worker_count == 1 {
            let (mut reader, mut attr_reader) = self.buffered_readers_for_options(&options)?;
            for chunk in chunks {
                let result = work_chunk(self, chunk, &options, &mut reader, &mut attr_reader)?;
                visit(result)?;
            }
            return Ok(());
        }

        let source = reusable_parallel_volume_source(self.volume)?;
        let next_index = AtomicUsize::new(0);
        let chunk_count = chunks.len();
        let chunks = chunks.into_boxed_slice();
        let boot = self.boot.clone();
        let extent_map = Arc::clone(&self.extent_map);
        let bitmap = Arc::clone(&self.bitmap);

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
                let extent_map = Arc::clone(&extent_map);
                let bitmap = Arc::clone(&bitmap);
                let work_chunk = &work_chunk;
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
                    let (mut reader, mut attr_reader) =
                        match worker_mft.buffered_readers_for_options(&options) {
                            Ok(readers) => readers,
                            Err(error) => {
                                let _ = tx.send(Err(error));
                                return;
                            }
                        };

                    loop {
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        if index >= chunks.len() {
                            break;
                        }

                        let chunk = chunks[index];
                        let result = work_chunk(
                            &worker_mft,
                            chunk,
                            &options,
                            &mut reader,
                            &mut attr_reader,
                        )
                        .map(|result| (index, result));
                        if tx.send(result).is_err() {
                            break;
                        }
                    }
                }));
            }
            drop(tx);

            drain_parallel_results_in_order(rx, chunk_count, &mut visit)?;

            for handle in handles {
                if handle.join().is_err() {
                    return Err(worker_panicked());
                }
            }

            Ok(())
        })
    }
}

/// Drain worker results, buffering out-of-order completions until they can be
/// yielded in original chunk order.
fn drain_parallel_results_in_order<T, Visit>(
    rx: mpsc::Receiver<Result<(usize, T), UsnError>>,
    chunk_count: usize,
    visit: &mut Visit,
) -> Result<(), UsnError>
where
    Visit: FnMut(T) -> Result<(), UsnError>,
{
    let mut next_expected = 0usize;
    let mut pending = Vec::with_capacity(chunk_count);
    pending.resize_with(chunk_count, || None);

    while next_expected < chunk_count {
        match rx.recv() {
            Ok(Ok((index, result))) => {
                pending[index] = Some(result);
                while next_expected < chunk_count {
                    let Some(result) = pending[next_expected].take() else {
                        break;
                    };
                    visit(result)?;
                    next_expected += 1;
                }
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(channel_closed()),
        }
    }

    Ok(())
}

/// Resolve the original volume into a reopenable source for worker threads.
fn reusable_parallel_volume_source(volume: &Volume) -> Result<ParallelVolumeSource, UsnError> {
    volume
        .drive_letter()
        .map(ParallelVolumeSource::DriveLetter)
        .or_else(|| {
            volume
                .mount_point()
                .map(|path| ParallelVolumeSource::MountPoint(path.to_path_buf()))
        })
        .ok_or_else(|| {
            UsnError::Io(io::Error::other(
                "raw_mft parallel chunk parsing requires a reusable volume source",
            ))
        })
}

    /// Reopen the original volume source for one worker thread.
fn open_parallel_volume(source: &ParallelVolumeSource) -> Result<Volume, UsnError> {
    match source {
        ParallelVolumeSource::DriveLetter(drive_letter) => Volume::from_drive_letter(*drive_letter),
        ParallelVolumeSource::MountPoint(path) => Volume::from_mount_point(path),
    }
}

/// Build a stable error when available parallelism cannot be queried.
fn available_parallelism_error(error: io::Error) -> UsnError {
    UsnError::Io(io::Error::other(format!(
        "failed to query available parallelism: {error}"
    )))
}

/// Build the channel-closed error used by the ordered parallel executor.
fn channel_closed() -> UsnError {
    UsnError::Io(io::Error::other(
        "raw_mft parallel chunk channel closed unexpectedly",
    ))
}

/// Build the panic-propagation error used by the ordered parallel executor.
fn worker_panicked() -> UsnError {
    UsnError::Io(io::Error::other("raw_mft parallel worker panicked"))
}