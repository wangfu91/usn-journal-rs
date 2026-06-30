//! Cached two-phase `$ATTRIBUTE_LIST` enrichment for parallel raw-`$MFT` scans.
//!
//! `$ATTRIBUTE_LIST` enrichment must read the *extension* FILE records that a
//! base record's attributes spill into. Done inline, those reads are scattered
//! random I/O and dominate a whole-`$MFT` scan. But the scan already streams
//! past every extension record (it just skips them). This module captures those
//! records' raw bytes during the main pass, then enriches the (relatively few)
//! base records that need it from that in-memory cache — turning the random
//! extension reads into zero extra I/O.
//!
//! * **Phase 1** (parallel): each worker walks its chunks. Extension records
//!   (`base_reference != 0`) are cached by record number as raw, un-fixed-up
//!   bytes; base records that need enrichment are deferred; everything else is
//!   folded immediately.
//! * **Phase 2**: the deferred base records are enriched from the merged cache
//!   and folded.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::{
    errors::UsnError,
    raw_mft::{
        RawMft, RawMftWorkChunk,
        attr_list::{enrich_batch_from_attr_list_cached, should_enrich_batch_from_attr_list},
        entry_build::{AttributeListInfo, RawMftBatchEntry, RawMftBatchScratch},
        io::VolumeReader,
        options::RawMftScanOptions,
        serial::engine::{SerialParseState, for_each_record_capturing},
    },
};

use super::executor::{open_parallel_volume, reusable_parallel_volume_source};

/// A base record whose enrichment is deferred to phase 2.
struct DeferredBase {
    /// Partially built batch entry awaiting extension data.
    scratch: RawMftBatchScratch,
    /// The base record's captured `$ATTRIBUTE_LIST`.
    attr_list: AttributeListInfo,
    /// Base record number (used to skip self-references during enrichment).
    base_number: u64,
}

/// Per-worker phase-1 output.
struct WorkerOutput<T> {
    /// Folded non-deferred base entries.
    acc: T,
    /// Extension records captured by this worker (record number -> raw bytes).
    ext_cache: HashMap<u64, Box<[u8]>>,
    /// Base records this worker deferred for phase-2 enrichment.
    deferred: Vec<DeferredBase>,
}

impl RawMft<'_> {
    /// Fold every base FILE record into per-worker accumulators, deferring
    /// `$ATTRIBUTE_LIST` enrichment to a second phase that reads extension
    /// records from an in-memory cache captured during the scan instead of
    /// re-reading them randomly from disk.
    ///
    /// `init`/`fold`/`visit` mirror
    /// [`RawMftParallelScan::fold_chunks`](super::RawMftParallelScan::fold_chunks),
    /// except `init` takes no chunk (it is also used for the phase-2 accumulator).
    pub(crate) fn fold_records_cached_enrich<Init, Fold, Visit, T>(
        &self,
        chunks: Vec<RawMftWorkChunk>,
        options: RawMftScanOptions,
        worker_count: NonZeroUsize,
        init: Init,
        fold: Fold,
        mut visit: Visit,
    ) -> Result<(), UsnError>
    where
        Init: Fn() -> T + Sync,
        Fold: Fn(&mut T, RawMftBatchEntry) -> Result<(), UsnError> + Sync,
        Visit: FnMut(T) -> Result<(), UsnError>,
        T: Send,
    {
        if chunks.is_empty() {
            return Ok(());
        }
        let collect_dos = options.entry.collect_dos_file_name_links;
        let worker_count = worker_count.get().min(chunks.len()).max(1);

        // Phase 1: parallel capture. Workers reopen their own volume handle (the
        // raw `HANDLE` is not `Send`) and share only `Send` state.
        let source = reusable_parallel_volume_source(self.volume())?;
        let boot = self.boot.clone();
        let extent_map = std::sync::Arc::clone(&self.extent_map);
        let bitmap = std::sync::Arc::clone(&self.bitmap);
        let chunks = chunks.into_boxed_slice();
        let next_chunk = AtomicUsize::new(0);

        let outputs: Vec<WorkerOutput<T>> = thread::scope(|scope| -> Result<_, UsnError> {
            let mut handles = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                let next_chunk = &next_chunk;
                let chunks = &chunks;
                let source = source.clone();
                let boot = boot.clone();
                let extent_map = std::sync::Arc::clone(&extent_map);
                let bitmap = std::sync::Arc::clone(&bitmap);
                let options = options.clone();
                let init = &init;
                let fold = &fold;
                handles.push(scope.spawn(move || -> Result<WorkerOutput<T>, UsnError> {
                    let volume = open_parallel_volume(&source)?;
                    let worker_mft = RawMft {
                        volume: &volume,
                        boot,
                        extent_map,
                        bitmap,
                    };
                    worker_mft.capture_worker(
                        chunks,
                        next_chunk,
                        &options,
                        collect_dos,
                        init,
                        fold,
                    )
                }));
            }

            let mut outputs = Vec::with_capacity(worker_count);
            for handle in handles {
                match handle.join() {
                    Ok(result) => outputs.push(result?),
                    Err(_) => {
                        return Err(UsnError::Io(std::io::Error::other(
                            "raw_mft cached-enrich worker panicked",
                        )));
                    }
                }
            }
            Ok(outputs)
        })?;

        // Merge the per-worker extension caches and collect deferred bases.
        let mut ext_cache: HashMap<u64, Box<[u8]>> = HashMap::new();
        let mut deferred: Vec<DeferredBase> = Vec::new();
        for output in &outputs {
            deferred.reserve(output.deferred.len());
        }
        let mut worker_accs = Vec::with_capacity(outputs.len());
        for output in outputs {
            for (record_number, bytes) in output.ext_cache {
                ext_cache.entry(record_number).or_insert(bytes);
            }
            deferred.extend(output.deferred);
            worker_accs.push(output.acc);
        }

        // Phase 2: enrich the deferred base records from the cache. Measured
        // best single-threaded: it is dominated by parsing cached records, and
        // adding workers only adds contending volume reopens.
        let phase2_acc =
            self.enrich_deferred(deferred, &ext_cache, &options, collect_dos, &init, &fold)?;

        for acc in worker_accs {
            visit(acc)?;
        }
        visit(phase2_acc)?;
        Ok(())
    }

    /// One phase-1 worker: pull chunks, capturing extension records and deferring
    /// enrichment.
    fn capture_worker<Init, Fold, T>(
        &self,
        chunks: &[RawMftWorkChunk],
        next_chunk: &AtomicUsize,
        options: &RawMftScanOptions,
        collect_dos: bool,
        init: &Init,
        fold: &Fold,
    ) -> Result<WorkerOutput<T>, UsnError>
    where
        Init: Fn() -> T,
        Fold: Fn(&mut T, RawMftBatchEntry) -> Result<(), UsnError>,
    {
        let mut acc = init();
        let mut ext_cache: HashMap<u64, Box<[u8]>> = HashMap::new();
        let mut deferred: Vec<DeferredBase> = Vec::new();
        let mut reader =
            VolumeReader::with_buffer_bytes(self.volume().handle, self.boot.bytes_per_sector as u64, options.buffers.main.get())?;

        loop {
            let index = next_chunk.fetch_add(1, Ordering::Relaxed);
            if index >= chunks.len() {
                break;
            }
            let chunk = chunks[index];
            let mut state =
                SerialParseState::for_range(self, options, chunk.start_record, chunk.end_record);
            for_each_record_capturing(
                self,
                &mut state,
                &mut reader,
                |record| {
                    let (scratch, attr_list) =
                        RawMftBatchScratch::from_record_with_attr_list(record, collect_dos);
                    if let Some(attr_list) = attr_list
                        && should_enrich_batch_from_attr_list(&scratch)
                    {
                        deferred.push(DeferredBase {
                            scratch,
                            attr_list,
                            base_number: record.number,
                        });
                    } else {
                        fold(&mut acc, scratch.into_entry())?;
                    }
                    Ok(())
                },
                |record_number, raw| {
                    ext_cache
                        .entry(record_number)
                        .or_insert_with(|| raw.to_vec().into_boxed_slice());
                },
            )?;
        }

        Ok(WorkerOutput {
            acc,
            ext_cache,
            deferred,
        })
    }

    /// Phase 2: enrich every deferred base record from the captured cache and
    /// fold the results into one accumulator.
    ///
    /// Runs single-threaded: the work is dominated by parsing the cached
    /// extension records, and a worker-count sweep showed parallelism only adds
    /// contending volume reopens (1w 0.41s, 8w 0.60s, 16w 1.11s).
    fn enrich_deferred<Init, Fold, T>(
        &self,
        deferred: Vec<DeferredBase>,
        ext_cache: &HashMap<u64, Box<[u8]>>,
        options: &RawMftScanOptions,
        collect_dos: bool,
        init: &Init,
        fold: &Fold,
    ) -> Result<T, UsnError>
    where
        Init: Fn() -> T,
        Fold: Fn(&mut T, RawMftBatchEntry) -> Result<(), UsnError>,
    {
        let mut acc = init();
        // Only needed for the rare non-resident `$ATTRIBUTE_LIST` payload.
        let mut attr_reader = VolumeReader::with_buffer_bytes(
            self.volume().handle,
            self.boot.bytes_per_sector as u64,
            options.buffers.attr.get(),
        )?;

        for deferred in deferred {
            let mut scratch = deferred.scratch;
            enrich_batch_from_attr_list_cached(
                &mut scratch,
                deferred.attr_list,
                deferred.base_number,
                &mut attr_reader,
                &self.boot,
                collect_dos,
                ext_cache,
            );
            fold(&mut acc, scratch.into_entry())?;
        }
        Ok(acc)
    }
}
