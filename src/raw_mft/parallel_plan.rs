//! Builder-style public facade for parallel raw-MFT chunk scans.

use std::{num::NonZeroUsize, thread};

use crate::{
    errors::UsnError,
    raw_mft::{
        RawMft, RawMftBatchEntry, RawMftChunkBatch, RawMftChunkPlanOptions, RawMftScanOptions,
        RawMftWorkChunk, parallel_executor,
    },
};

/// Configured parallel raw `$MFT` scan.
#[derive(Clone)]
#[must_use]
pub struct RawMftParallelScan<'m, 'v> {
    mft: &'m RawMft<'v>,
    chunk_plan: RawMftChunkPlanOptions,
    chunks: Option<Vec<RawMftWorkChunk>>,
    scan_options: RawMftScanOptions,
    worker_count: Option<NonZeroUsize>,
}

impl<'m, 'v> RawMftParallelScan<'m, 'v> {
    pub(crate) fn new(mft: &'m RawMft<'v>) -> Self {
        Self {
            mft,
            chunk_plan: RawMftChunkPlanOptions::default(),
            chunks: None,
            scan_options: RawMftScanOptions::default(),
            worker_count: None,
        }
    }

    /// Use a custom chunk planning policy when explicit chunks are not provided.
    pub fn chunk_plan(mut self, options: RawMftChunkPlanOptions) -> Self {
        self.chunk_plan = options;
        self.chunks = None;
        self
    }

    /// Use explicit chunks instead of planning them from this scan.
    pub fn chunks(mut self, chunks: Vec<RawMftWorkChunk>) -> Self {
        self.chunks = Some(chunks);
        self
    }

    /// Use custom raw-MFT scan options for each chunk.
    pub fn scan_options(mut self, options: RawMftScanOptions) -> Self {
        self.scan_options = options;
        self
    }

    /// Use a fixed worker count. Defaults to `thread::available_parallelism()`.
    pub fn workers(mut self, worker_count: NonZeroUsize) -> Self {
        self.worker_count = Some(worker_count);
        self
    }

    /// Parse chunks in parallel and collect ordered batches.
    pub fn collect_batches(self) -> Result<Vec<RawMftChunkBatch>, UsnError> {
        let worker_count = self.resolved_worker_count()?;
        self.mft
            .read_chunks(self.resolved_chunks(), self.scan_options, worker_count)
    }

    /// Parse chunks in parallel and visit ordered batches.
    pub fn for_each_batch<F>(self, visit: F) -> Result<(), UsnError>
    where
        F: FnMut(RawMftChunkBatch) -> Result<(), UsnError>,
    {
        let worker_count = self.resolved_worker_count()?;
        self.mft.for_each_chunk(
            self.resolved_chunks(),
            self.scan_options,
            worker_count,
            visit,
        )
    }

    /// Parse chunks in parallel, map each batch on the worker thread, and visit mapped values in chunk order.
    pub fn map_chunks<F, T, V>(self, map_chunk: F, visit: V) -> Result<(), UsnError>
    where
        F: Fn(RawMftChunkBatch) -> Result<T, UsnError> + Sync,
        T: Send,
        V: FnMut(T) -> Result<(), UsnError>,
    {
        let worker_count = self.resolved_worker_count()?;
        self.mft.for_each_mapped_chunk(
            self.resolved_chunks(),
            self.scan_options,
            worker_count,
            map_chunk,
            visit,
        )
    }

    /// Parse chunks in parallel and fold entries into one worker-local accumulator per chunk.
    pub fn fold_chunks<Init, Fold, T, V>(
        self,
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
        let worker_count = self.resolved_worker_count()?;
        self.mft.for_each_folded_chunk(
            self.resolved_chunks(),
            self.scan_options,
            worker_count,
            init,
            fold_entry,
            visit,
        )
    }

    fn resolved_chunks(&self) -> Vec<RawMftWorkChunk> {
        self.chunks
            .clone()
            .unwrap_or_else(|| self.mft.plan_chunks_with_options(self.chunk_plan.clone()))
    }

    fn resolved_worker_count(&self) -> Result<NonZeroUsize, UsnError> {
        match self.worker_count {
            Some(worker_count) => Ok(worker_count),
            None => thread::available_parallelism()
                .map_err(parallel_executor::available_parallelism_error),
        }
    }
}

impl<'v> RawMft<'v> {
    /// Configure a parallel raw `$MFT` chunk scan.
    pub fn parallel(&self) -> RawMftParallelScan<'_, 'v> {
        RawMftParallelScan::new(self)
    }
}
