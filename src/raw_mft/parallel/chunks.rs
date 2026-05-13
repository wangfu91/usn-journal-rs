//! Chunk planning and chunk-local operations for parallel raw-MFT scans.

use std::{num::NonZeroUsize, time::Instant};

use log::debug;

use crate::{
    errors::UsnError,
    raw_mft::{
        RawMft,
        attr_list::{
            PreparedBatchAttrListEnrichment, apply_prepared_batch_attr_list_enrichment,
            enrich_batch_from_attr_list, enrich_batch_from_attr_list_for_summary,
            prepare_batch_attr_list_enrichment, should_enrich_batch_from_attr_list,
            should_enrich_batch_from_attr_list_for_summary,
        },
        attr_list_profile,
        chunk_plan::{self, RawMftChunkPlanOptions, RawMftWorkChunk},
        entry_build::{RawMftBatchEntry, RawMftBatchScratch, RawMftChunkBatch},
        io::VolumeReader,
        options::AttrListBatchMode,
        options::RawMftScanOptions,
        reader::read_batch_record_raw,
        serial::engine::{SerialParseState, next_record_output},
    },
};

use super::ChunkScheduling;
use super::executor;

enum ParsedBatchRecord {
    Ready(RawMftBatchEntry),
    Deferred(PreparedBatchAttrListEnrichment),
}

enum ChunkBatchEntrySlot {
    Ready(RawMftBatchEntry),
    Deferred(usize),
}

struct ScheduledBatchExtensionLoad {
    ordinal: usize,
    offset_key: u64,
    task_index: usize,
    ext_index: usize,
    ext_record_number: u64,
}

impl<'a> RawMft<'a> {
    /// Build deterministic logical work chunks for raw `$MFT` parsing.
    #[must_use]
    pub fn plan_chunks(&self) -> Vec<RawMftWorkChunk> {
        self.plan_chunks_with_options(RawMftChunkPlanOptions::default())
    }

    /// Build logical work chunks with custom planning options.
    #[must_use]
    pub fn plan_chunks_with_options(
        &self,
        options: RawMftChunkPlanOptions,
    ) -> Vec<RawMftWorkChunk> {
        let range = options.range();
        let end_record = range
            .end_record()
            .unwrap_or(self.record_count())
            .min(self.record_count());
        chunk_plan::build_work_chunks(
            range.start_record(),
            end_record,
            options.max_records_per_chunk(),
            options.skip_unused(),
            |record_number| self.bitmap_used(record_number),
        )
    }

    /// Parse one logical work chunk into lean batch entries.
    pub fn read_chunk(
        &self,
        chunk: RawMftWorkChunk,
        options: RawMftScanOptions,
    ) -> Result<Vec<RawMftBatchEntry>, UsnError> {
        let (mut reader, mut attr_reader) = self.buffered_readers_for_options(&options)?;
        self.read_chunk_with_reused_readers(chunk, &options, &mut reader, &mut attr_reader)
    }

    /// Parse one logical work chunk and fold lean batch entries into a caller-owned accumulator.
    pub fn fold_chunk<T, Init, Fold>(
        &self,
        chunk: RawMftWorkChunk,
        options: RawMftScanOptions,
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
        options: &RawMftScanOptions,
        reader: &mut VolumeReader,
        attr_reader: &mut VolumeReader,
        mut visit: F,
    ) -> Result<(), UsnError>
    where
        F: FnMut(RawMftBatchEntry) -> Result<(), UsnError>,
    {
        let mut scan = SerialParseState::for_range(self, options, start_record, end_record);
        let deferred_attr_list = options.deferred_chunk_attr_list_enrichment();
        let attr_list_batch_mode = options.attr_list_batch_mode();
        let collect_dos_file_name_links = options.entry.collect_dos_file_name_links;
        let sort_attr_list_extensions_by_offset = options.sort_attr_list_extensions_by_offset();
        let mut entry_slots = Vec::new();
        let mut deferred_tasks = Vec::new();
        let mut window_record_count = 0usize;
        let deferred_window_limit = options.deferred_chunk_attr_list_window_records();

        while let Some(entry) = next_record_output(self, &mut scan, reader, |record| {
            let record_number = record.number;
            attr_list_profile::record_scanned_record();

            let (entry, attr_list) = RawMftBatchScratch::from_record_with_attr_list(
                record,
                collect_dos_file_name_links,
            );

            if let Some(attr_list) = attr_list {
                attr_list_profile::record_attr_list_present(&attr_list);
                let should_enrich = match attr_list_batch_mode {
                    AttrListBatchMode::Full => should_enrich_batch_from_attr_list(&entry),
                    AttrListBatchMode::SummaryOnly => {
                        should_enrich_batch_from_attr_list_for_summary(&entry)
                    }
                };
                attr_list_profile::record_need_check(should_enrich);

                if should_enrich {
                    if !deferred_attr_list {
                        attr_list_profile::set_current_enrichment_base_context(
                            record.volume_offset,
                            start_record,
                            end_record,
                        );
                        let mut entry = entry;
                        if attr_list_profile::is_enabled() {
                            let started = Instant::now();
                            let stats = match attr_list_batch_mode {
                                AttrListBatchMode::Full => enrich_batch_from_attr_list(
                                    &mut entry,
                                    attr_list,
                                    record_number,
                                    attr_reader,
                                    &self.boot,
                                    self.extent_map.as_ref(),
                                    collect_dos_file_name_links,
                                    sort_attr_list_extensions_by_offset,
                                ),
                                AttrListBatchMode::SummaryOnly => {
                                    enrich_batch_from_attr_list_for_summary(
                                        &mut entry,
                                        attr_list,
                                        record_number,
                                        attr_reader,
                                        &self.boot,
                                        self.extent_map.as_ref(),
                                        collect_dos_file_name_links,
                                        sort_attr_list_extensions_by_offset,
                                    )
                                }
                            };
                            attr_list_profile::record_enrichment(stats, started.elapsed());
                        } else {
                            match attr_list_batch_mode {
                                AttrListBatchMode::Full => {
                                    let _ = enrich_batch_from_attr_list(
                                        &mut entry,
                                        attr_list,
                                        record_number,
                                        attr_reader,
                                        &self.boot,
                                        self.extent_map.as_ref(),
                                        collect_dos_file_name_links,
                                        sort_attr_list_extensions_by_offset,
                                    );
                                }
                                AttrListBatchMode::SummaryOnly => {
                                    let _ = enrich_batch_from_attr_list_for_summary(
                                        &mut entry,
                                        attr_list,
                                        record_number,
                                        attr_reader,
                                        &self.boot,
                                        self.extent_map.as_ref(),
                                        collect_dos_file_name_links,
                                        sort_attr_list_extensions_by_offset,
                                    );
                                }
                            }
                        }

                        return Ok(ParsedBatchRecord::Ready(entry.into_entry()));
                    }

                    return Ok(ParsedBatchRecord::Deferred(
                        prepare_batch_attr_list_enrichment(
                            entry,
                            attr_list,
                            record_number,
                            record.volume_offset,
                            attr_reader,
                            &self.boot,
                            attr_list_batch_mode,
                        ),
                    ));
                }
            }

            Ok(ParsedBatchRecord::Ready(entry.into_entry()))
        })? {
            if !deferred_attr_list {
                match entry {
                    ParsedBatchRecord::Ready(entry) => visit(entry)?,
                    ParsedBatchRecord::Deferred(task) => {
                        let task_index = deferred_tasks.len();
                        deferred_tasks.push(Some(task));
                        entry_slots.push(ChunkBatchEntrySlot::Deferred(task_index));
                        flush_batch_entry_window(
                            self,
                            options,
                            start_record,
                            end_record,
                            attr_reader,
                            &mut entry_slots,
                            &mut deferred_tasks,
                            &mut visit,
                        )?;
                    }
                }
                continue;
            }

            match entry {
                ParsedBatchRecord::Ready(entry) => {
                    if deferred_tasks.is_empty() {
                        visit(entry)?;
                        continue;
                    }
                    entry_slots.push(ChunkBatchEntrySlot::Ready(entry));
                }
                ParsedBatchRecord::Deferred(task) => {
                    let task_index = deferred_tasks.len();
                    deferred_tasks.push(Some(task));
                    entry_slots.push(ChunkBatchEntrySlot::Deferred(task_index));
                }
            }

            if !deferred_tasks.is_empty() {
                window_record_count += 1;
                if window_record_count >= deferred_window_limit {
                    flush_batch_entry_window(
                        self,
                        options,
                        start_record,
                        end_record,
                        attr_reader,
                        &mut entry_slots,
                        &mut deferred_tasks,
                        &mut visit,
                    )?;
                    window_record_count = 0;
                }
            }
        }

        if deferred_attr_list && (!entry_slots.is_empty() || !deferred_tasks.is_empty()) {
            flush_batch_entry_window(
                self,
                options,
                start_record,
                end_record,
                attr_reader,
                &mut entry_slots,
                &mut deferred_tasks,
                &mut visit,
            )?;
        }

        Ok(())
    }

    /// Parse one chunk into batch entries using caller-supplied readers.
    fn read_chunk_with_reused_readers(
        &self,
        chunk: RawMftWorkChunk,
        options: &RawMftScanOptions,
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
        options: &RawMftScanOptions,
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

    /// Parse logical work chunks in parallel, transform them on worker threads, and visit results
    /// in deterministic chunk order.
    pub(crate) fn for_each_mapped_chunk<F, T, V>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftScanOptions,
        worker_count: NonZeroUsize,
        scheduling: ChunkScheduling,
        map_chunk: F,
        visit: V,
    ) -> Result<(), UsnError>
    where
        F: Fn(RawMftChunkBatch) -> Result<T, UsnError> + Sync,
        T: Send,
        V: FnMut(T) -> Result<(), UsnError>,
    {
        executor::run_parallel_chunks_in_order(
            self,
            chunks,
            options,
            worker_count,
            scheduling,
            move |mft, chunk, options, reader, attr_reader| {
                mft.read_chunk_with_reused_readers(chunk, options, reader, attr_reader)
                    .and_then(|entries| map_chunk(RawMftChunkBatch { chunk, entries }))
            },
            visit,
        )
    }

    /// Parse logical work chunks in parallel and fold lean batch entries on worker threads.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_each_folded_chunk<Init, Fold, T, V>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftScanOptions,
        worker_count: NonZeroUsize,
        scheduling: ChunkScheduling,
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
        executor::run_parallel_chunks_in_order(
            self,
            chunks,
            options,
            worker_count,
            scheduling,
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
    pub(crate) fn for_each_chunk<F>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftScanOptions,
        worker_count: NonZeroUsize,
        scheduling: ChunkScheduling,
        visit: F,
    ) -> Result<(), UsnError>
    where
        F: FnMut(RawMftChunkBatch) -> Result<(), UsnError>,
    {
        self.for_each_mapped_chunk(
            chunks,
            options,
            worker_count,
            scheduling,
            Ok::<_, UsnError>,
            visit,
        )
    }

    /// Parse logical work chunks in parallel using worker-local readers and custom options.
    pub(crate) fn read_chunks(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftScanOptions,
        worker_count: NonZeroUsize,
        scheduling: ChunkScheduling,
    ) -> Result<Vec<RawMftChunkBatch>, UsnError> {
        let mut ordered_batches = Vec::with_capacity(chunks.len());
        self.for_each_chunk(chunks, options, worker_count, scheduling, |batch| {
            ordered_batches.push(batch);
            Ok(())
        })?;
        Ok(ordered_batches)
    }
}

fn flush_batch_entry_window<F>(
    mft: &RawMft<'_>,
    options: &RawMftScanOptions,
    start_record: u64,
    end_record: u64,
    attr_reader: &mut VolumeReader,
    entry_slots: &mut Vec<ChunkBatchEntrySlot>,
    deferred_tasks: &mut Vec<Option<PreparedBatchAttrListEnrichment>>,
    visit: &mut F,
) -> Result<(), UsnError>
where
    F: FnMut(RawMftBatchEntry) -> Result<(), UsnError>,
{
    if !deferred_tasks.is_empty() {
        let deferred_started = Instant::now();
        let mut load_plan = build_scheduled_batch_extension_loads(
            deferred_tasks,
            mft.extent_map.as_ref(),
            options.sort_attr_list_extensions_by_offset(),
        );
        for scheduled in load_plan.drain(..) {
            let Some(task) = deferred_tasks[scheduled.task_index].as_mut() else {
                continue;
            };
            attr_list_profile::set_current_enrichment_base_context(
                task.base_record_offset,
                start_record,
                end_record,
            );
            let load_started = Instant::now();
            let _extension_scope = attr_list_profile::enter_extension_load_scope();
            let load_result = read_batch_record_raw(
                attr_reader,
                &mft.boot,
                mft.extent_map.as_ref(),
                scheduled.ext_record_number,
                options.entry.collect_dos_file_name_links,
            )
            .map(|result| result.map(|(entry, _)| entry));
            attr_list_profile::record_extension_record_load_attempt(load_started.elapsed());
            match load_result {
                Ok(Some(ext_entry)) => {
                    task.loaded_extensions[scheduled.ext_index] = Some(ext_entry)
                }
                Ok(None) => {}
                Err(error) => {
                    debug!(
                        "raw_mft: record {}: failed to load extension record {}: {}",
                        task.base_record_number, scheduled.ext_record_number, error,
                    );
                }
            }
        }
        for task in deferred_tasks.iter_mut().flatten() {
            let stats = apply_prepared_batch_attr_list_enrichment(task);
            attr_list_profile::record_enrichment(stats, std::time::Duration::ZERO);
        }
        attr_list_profile::record_enrichment_wall_time(deferred_started.elapsed());
    }

    for slot in entry_slots.drain(..) {
        match slot {
            ChunkBatchEntrySlot::Ready(entry) => visit(entry)?,
            ChunkBatchEntrySlot::Deferred(task_index) => {
                let task = deferred_tasks[task_index]
                    .take()
                    .expect("deferred batch task should still be present");
                visit(task.entry.into_entry())?;
            }
        }
    }
    deferred_tasks.clear();
    Ok(())
}

fn build_scheduled_batch_extension_loads(
    deferred_tasks: &[Option<PreparedBatchAttrListEnrichment>],
    extent_map: &crate::raw_mft::layout::extent::ExtentMap,
    sort_by_offset: bool,
) -> Vec<ScheduledBatchExtensionLoad> {
    let mut load_plan = Vec::new();
    for (task_index, task) in deferred_tasks.iter().enumerate() {
        let Some(task) = task.as_ref() else {
            continue;
        };
        for (ext_index, &ext_record_number) in task.ext_records.iter().enumerate() {
            let offset_key = if sort_by_offset {
                match extent_map.record_offset(ext_record_number) {
                    Ok(Some(offset)) => offset,
                    Ok(None) => u64::MAX - 1,
                    Err(_) => u64::MAX,
                }
            } else {
                0
            };
            load_plan.push(ScheduledBatchExtensionLoad {
                ordinal: load_plan.len(),
                offset_key,
                task_index,
                ext_index,
                ext_record_number,
            });
        }
    }
    if sort_by_offset {
        load_plan.sort_unstable_by_key(|scheduled| (scheduled.offset_key, scheduled.ordinal));
    }
    load_plan
}
