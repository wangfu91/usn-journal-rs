//! Worker/executor internals for deterministic parallel raw-MFT chunk scans.

use std::{
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
        Mutex,
    },
    thread,
};

use crate::{
    errors::UsnError,
    raw_mft::{
        RawMft, RawMftWorkChunk,
        attr_list::{BatchAttrListHint, estimate_batch_attr_list_hint},
        io::VolumeReader,
        layout::extent::ExtentMap,
        options::RawMftScanOptions,
        reader::read_batch_record_raw,
        schedule_profile,
    },
    volume::Volume,
};

/// Internal worker scheduling mode for chunk execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkScheduling {
    /// Workers fetch the next remaining chunk from a shared atomic cursor.
    Dynamic,
    /// Workers fetch the next remaining chunk from a shared atomic cursor after the chunk list has been reordered by physical start offset.
    DynamicPhysicalOrder,
    /// Workers fetch the next remaining chunk from a shared atomic cursor after the chunk list has been grouped into local physical bands and each band has been front-loaded by an estimated chunk cost heuristic.
    DynamicCostBanded,
    /// Workers fetch chunks from local physical bands, but later bands are reordered using elapsed-time feedback from already completed chunks.
    DynamicObservedAdaptive,
    /// Each worker receives one contiguous band of chunk indices.
    Contiguous,
}

const COST_AWARE_BAND_WAVES: usize = 4;
const MIN_COST_AWARE_BAND_CHUNKS: usize = 8;
const COST_HINT_SAMPLE_RECORDS: usize = 16;
const OBSERVED_ADAPTIVE_RIDGE_LAMBDA: f64 = 0.25;
const OBSERVED_ADAPTIVE_FEATURE_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ChunkAttrListHintSummary {
    sampled_records: u16,
    attr_list_records: u16,
    nonresident_attr_lists: u16,
    enrich_candidates: u16,
    referenced_extension_records: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct ChunkEstimatedCost {
    used_records: u16,
    usage_transitions: u16,
    attr_list_records: u16,
    nonresident_attr_lists: u16,
    enrich_candidates: u16,
    referenced_extension_records: u16,
    sparse_segments: u16,
    discontinuities: u16,
    overlapped_segments: u16,
    physical_span_bytes: u64,
    covered_bytes: u64,
}

impl ChunkEstimatedCost {
    fn score(self) -> u64 {
        let span_mib = self.physical_span_bytes / (1024 * 1024);
        (self.referenced_extension_records as u64) * 4_096
            + (self.enrich_candidates as u64) * 2_048
            + (self.attr_list_records as u64) * 1_024
            + (self.nonresident_attr_lists as u64) * 256
            + (self.used_records as u64) * 64
            + (self.usage_transitions as u64) * 32
            + (self.discontinuities as u64) * 16
            + (self.sparse_segments as u64) * 8
            + (self.overlapped_segments as u64) * 4
            + span_mib.min(4_096)
    }

    fn into_profile(self) -> schedule_profile::ChunkCostProfile {
        schedule_profile::ChunkCostProfile {
            used_records: self.used_records,
            usage_transitions: self.usage_transitions,
            attr_list_records: self.attr_list_records,
            nonresident_attr_lists: self.nonresident_attr_lists,
            enrich_candidates: self.enrich_candidates,
            referenced_extension_records: self.referenced_extension_records,
            sparse_segments: self.sparse_segments,
            discontinuities: self.discontinuities,
            overlapped_segments: self.overlapped_segments,
            physical_span_bytes: self.physical_span_bytes,
            covered_bytes: self.covered_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkExecutionMeta {
    index: usize,
    physical_order_key: (u64, u64, u64),
    estimated_cost: ChunkEstimatedCost,
    claim_order: usize,
    band_index: usize,
    band_position: usize,
    prediction_source: schedule_profile::PredictionSource,
    predicted_order_key: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ObservedChunkSample {
    features: [f64; OBSERVED_ADAPTIVE_FEATURE_COUNT],
    elapsed_ms: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ObservedCostModel {
    intercept_ms: f64,
    coefficients_ms: [f64; OBSERVED_ADAPTIVE_FEATURE_COUNT],
    observed_min_ms: f64,
    observed_median_ms: f64,
    observed_p90_ms: f64,
    observed_max_ms: f64,
}

#[derive(Debug)]
struct AdaptiveBandScheduler {
    state: Mutex<AdaptiveBandSchedulerState>,
}

#[derive(Debug)]
struct AdaptiveBandSchedulerState {
    bands: Vec<Vec<ChunkExecutionMeta>>,
    prepared: Vec<bool>,
    current_band: usize,
    next_in_band: usize,
    next_claim_order: usize,
    wave_size: usize,
    min_samples: usize,
    samples: Vec<ObservedChunkSample>,
}

/// Reopenable source information for worker-local volume handles.
#[derive(Debug, Clone)]
enum ParallelVolumeSource {
    DriveLetter(char),
    MountPoint(PathBuf),
}

/// Run chunk work in parallel and visit results in original chunk order.
pub(super) fn run_parallel_chunks_in_order<T, Work, Visit>(
    mft: &RawMft<'_>,
    chunks: Vec<RawMftWorkChunk>,
    options: RawMftScanOptions,
    worker_count: NonZeroUsize,
    scheduling: ChunkScheduling,
    work_chunk: Work,
    mut visit: Visit,
) -> Result<(), UsnError>
where
    for<'m> Work: Fn(
            &RawMft<'m>,
            RawMftWorkChunk,
            &RawMftScanOptions,
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
        let (mut reader, mut attr_reader) = mft.buffered_readers_for_options(&options)?;
        for chunk in chunks {
            let result = work_chunk(mft, chunk, &options, &mut reader, &mut attr_reader)?;
            visit(result)?;
        }
        return Ok(());
    }

    let source = reusable_parallel_volume_source(mft.volume)?;
    let next_index = AtomicUsize::new(0);
    let chunk_count = chunks.len();
    let chunks = chunks.into_boxed_slice();
    let chunk_attr_list_hints = if matches!(scheduling, ChunkScheduling::DynamicCostBanded)
        && attr_list_sampling_cost_hints_enabled()
    {
        Some(sample_chunk_attr_list_hints(mft, &chunks, &options)?)
    } else {
        None
    };
    let chunk_costs = estimate_chunk_costs(
        &chunks,
        mft.extent_map.as_ref(),
        mft.bitmap.as_ref(),
        chunk_attr_list_hints.as_deref(),
    );
    let adaptive_scheduler = matches!(scheduling, ChunkScheduling::DynamicObservedAdaptive).then(|| {
        Arc::new(AdaptiveBandScheduler::new(
            &chunks,
            mft.extent_map.as_ref(),
            &chunk_costs,
            worker_count,
        ))
    });
    let execution_order = dynamic_execution_order(
        &chunks,
        mft.extent_map.as_ref(),
        mft.bitmap.as_ref(),
        &chunk_costs,
        scheduling,
        worker_count,
    );
    let boot = mft.boot.clone();
    let extent_map = Arc::clone(&mft.extent_map);
    let bitmap = Arc::clone(&mft.bitmap);

    thread::scope(|scope| -> Result<(), UsnError> {
        let (tx, rx) = mpsc::channel::<Result<(usize, T), UsnError>>();
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let next_index = &next_index;
            let chunks = &chunks;
            let chunk_costs = &chunk_costs;
            let execution_order = &execution_order;
            let adaptive_scheduler = adaptive_scheduler.as_ref().map(Arc::clone);
            let tx = tx.clone();
            let options = options.clone();
            let source = source.clone();
            let boot = boot.clone();
            let extent_map = Arc::clone(&extent_map);
            let bitmap = Arc::clone(&bitmap);
            let work_chunk = &work_chunk;
            handles.push(scope.spawn(move || {
                schedule_profile::ensure_worker_slot(worker_index);
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

                match scheduling {
                    ChunkScheduling::Dynamic
                    | ChunkScheduling::DynamicPhysicalOrder
                    | ChunkScheduling::DynamicCostBanded => loop {
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        if index >= execution_order.len() {
                            break;
                        }

                        let chunk_index = execution_order[index];
                        let chunk = chunks[chunk_index];
                        let estimated_cost = chunk_costs[chunk_index];
                        let physical_start_offset =
                            chunk_physical_start_offset(chunk, worker_mft.extent_map.as_ref());
                        let started = std::time::Instant::now();
                        let result = work_chunk(
                            &worker_mft,
                            chunk,
                            &options,
                            &mut reader,
                            &mut attr_reader,
                        )
                        .map(|result| {
                            schedule_profile::record_chunk_completion(
                                worker_index,
                                chunk_index,
                                chunk,
                                estimated_cost.into_profile(),
                                index,
                                None,
                                None,
                                schedule_profile::PredictionSource::StaticEstimate,
                                estimated_cost.score(),
                                physical_start_offset,
                                started.elapsed(),
                            );
                            (chunk_index, result)
                        });
                        if tx.send(result).is_err() {
                            break;
                        }
                    },
                    ChunkScheduling::DynamicObservedAdaptive => loop {
                        let Some(scheduler) = adaptive_scheduler.as_ref() else {
                            let _ = tx.send(Err(channel_closed()));
                            break;
                        };
                        let Some(meta) = scheduler.claim_next() else {
                            break;
                        };

                        let chunk = chunks[meta.index];
                        let estimated_cost = meta.estimated_cost;
                        let physical_start_offset =
                            chunk_physical_start_offset(chunk, worker_mft.extent_map.as_ref());
                        let started = std::time::Instant::now();
                        let elapsed_and_result = work_chunk(
                            &worker_mft,
                            chunk,
                            &options,
                            &mut reader,
                            &mut attr_reader,
                        )
                        .map(|result| (started.elapsed(), result));
                        let result = elapsed_and_result.map(|(elapsed, result)| {
                            scheduler.record_completion(meta, elapsed);
                            schedule_profile::record_chunk_completion(
                                worker_index,
                                meta.index,
                                chunk,
                                estimated_cost.into_profile(),
                                meta.claim_order,
                                Some(meta.band_index),
                                Some(meta.band_position),
                                meta.prediction_source,
                                meta.predicted_order_key,
                                physical_start_offset,
                                elapsed,
                            );
                            (meta.index, result)
                        });
                        if tx.send(result).is_err() {
                            break;
                        }
                    },
                    ChunkScheduling::Contiguous => {
                        let (start, end) =
                            contiguous_worker_range(chunks.len(), worker_count, worker_index);
                        for index in start..end {
                            let chunk = chunks[index];
                            let estimated_cost = chunk_costs[index];
                            let physical_start_offset =
                                chunk_physical_start_offset(chunk, worker_mft.extent_map.as_ref());
                            let started = std::time::Instant::now();
                            let result = work_chunk(
                                &worker_mft,
                                chunk,
                                &options,
                                &mut reader,
                                &mut attr_reader,
                            )
                            .map(|result| {
                                schedule_profile::record_chunk_completion(
                                    worker_index,
                                    index,
                                    chunk,
                                    estimated_cost.into_profile(),
                                    index,
                                    None,
                                    None,
                                    schedule_profile::PredictionSource::StaticEstimate,
                                    estimated_cost.score(),
                                    physical_start_offset,
                                    started.elapsed(),
                                );
                                (index, result)
                            });
                            if tx.send(result).is_err() {
                                break;
                            }
                        }
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

fn dynamic_execution_order(
    chunks: &[RawMftWorkChunk],
    extent_map: &ExtentMap,
    bitmap: &[u8],
    chunk_costs: &[ChunkEstimatedCost],
    scheduling: ChunkScheduling,
    worker_count: usize,
) -> Box<[usize]> {
    let mut execution_order = (0..chunks.len()).collect::<Vec<_>>();
    match scheduling {
        ChunkScheduling::Dynamic => {}
        ChunkScheduling::DynamicPhysicalOrder => execution_order
            .sort_unstable_by_key(|&index| chunk_physical_order_key(chunks[index], extent_map)),
        ChunkScheduling::DynamicCostBanded => {
            return cost_aware_banded_execution_order(
                chunks,
                extent_map,
                bitmap,
                chunk_costs,
                worker_count,
            );
        }
        ChunkScheduling::DynamicObservedAdaptive => {}
        ChunkScheduling::Contiguous => {}
    }
    execution_order.into_boxed_slice()
}

impl AdaptiveBandScheduler {
    fn new(
        chunks: &[RawMftWorkChunk],
        extent_map: &ExtentMap,
        chunk_costs: &[ChunkEstimatedCost],
        worker_count: usize,
    ) -> Self {
        let mut execution = (0..chunks.len())
            .map(|index| ChunkExecutionMeta {
                index,
                physical_order_key: chunk_physical_order_key(chunks[index], extent_map),
                estimated_cost: chunk_costs[index],
                claim_order: 0,
                band_index: 0,
                band_position: 0,
                prediction_source: schedule_profile::PredictionSource::StaticEstimate,
                predicted_order_key: chunk_costs[index].score(),
            })
            .collect::<Vec<_>>();
        execution.sort_unstable_by_key(|meta| meta.physical_order_key);

        let band_size = cost_aware_band_size(worker_count, execution.len());
        let bands = execution
            .chunks(band_size)
            .enumerate()
            .map(|(band_index, band)| {
                band.iter()
                    .enumerate()
                    .map(|(band_position, meta)| ChunkExecutionMeta {
                        band_index,
                        band_position,
                        ..*meta
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let prepared = vec![false; bands.len()];

        Self {
            state: Mutex::new(AdaptiveBandSchedulerState {
                bands,
                prepared,
                current_band: 0,
                next_in_band: 0,
                next_claim_order: 0,
                wave_size: worker_count.max(1),
                min_samples: adaptive_min_samples(worker_count, band_size),
                samples: Vec::new(),
            }),
        }
    }

    fn claim_next(&self) -> Option<ChunkExecutionMeta> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };

        loop {
            if state.current_band >= state.bands.len() {
                return None;
            }

            let band_index = state.current_band;
            if !state.prepared[band_index] {
                let sample_count_before = state.samples.len();
                let model = (state.samples.len() >= state.min_samples)
                    .then(|| fit_observed_cost_model(&state.samples))
                    .filter(ObservedCostModel::is_informative);
                let wave_size = state.wave_size;
                let mut band_decision = None;
                if let Some(band) = state.bands.get_mut(band_index) {
                    order_band_for_execution(band, wave_size, model);
                    if let (Some(front), Some(back)) = (band.first(), band.last()) {
                        band_decision = Some((
                            band_index,
                            band.len(),
                            sample_count_before,
                            front.prediction_source,
                            front.index,
                            front.predicted_order_key,
                            back.index,
                            back.predicted_order_key,
                        ));
                    }
                    state.prepared[band_index] = true;
                }
                if let Some((
                    band_index,
                    chunk_count,
                    sample_count_before,
                    prediction_source,
                    front_chunk_index,
                    front_prediction_key,
                    back_chunk_index,
                    back_prediction_key,
                )) = band_decision
                {
                    schedule_profile::record_band_decision(
                        band_index,
                        chunk_count,
                        sample_count_before,
                        prediction_source,
                        front_chunk_index,
                        front_prediction_key,
                        back_chunk_index,
                        back_prediction_key,
                    );
                }
            }

            let Some(band) = state.bands.get(band_index) else {
                return None;
            };
            if let Some(mut meta) = band.get(state.next_in_band).copied() {
                state.next_in_band += 1;
                meta.claim_order = state.next_claim_order;
                state.next_claim_order = state.next_claim_order.saturating_add(1);
                return Some(meta);
            }

            state.current_band += 1;
            state.next_in_band = 0;
        }
    }

    fn record_completion(&self, meta: ChunkExecutionMeta, elapsed: std::time::Duration) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.samples.push(ObservedChunkSample {
            features: observed_cost_features(meta.estimated_cost),
            elapsed_ms: elapsed.as_secs_f64() * 1_000.0,
        });
    }
}

fn adaptive_min_samples(worker_count: usize, band_size: usize) -> usize {
    band_size.max(worker_count.max(1))
}

fn order_band_for_execution(
    band: &mut [ChunkExecutionMeta],
    wave_size: usize,
    model: Option<ObservedCostModel>,
) {
    let prediction_source = if model.is_some() {
        schedule_profile::PredictionSource::ObservedModel
    } else {
        schedule_profile::PredictionSource::StaticEstimate
    };
    band.sort_unstable_by(|left, right| {
        band_order_score(*right, model)
            .total_cmp(&band_order_score(*left, model))
            .then_with(|| left.physical_order_key.cmp(&right.physical_order_key))
    });
    for wave in band.chunks_mut(wave_size.max(1)) {
        wave.sort_unstable_by_key(|meta| meta.physical_order_key);
    }
    for (band_position, meta) in band.iter_mut().enumerate() {
        meta.band_position = band_position;
        meta.prediction_source = prediction_source;
        meta.predicted_order_key = band_order_key(*meta, model);
    }
}

fn band_order_score(meta: ChunkExecutionMeta, model: Option<ObservedCostModel>) -> f64 {
    model
        .map(|model| model.predict_ms(meta.estimated_cost))
        .unwrap_or_else(|| meta.estimated_cost.score() as f64)
}

fn band_order_key(meta: ChunkExecutionMeta, model: Option<ObservedCostModel>) -> u64 {
    model
        .map(|model| predicted_ms_to_key(model.predict_ms(meta.estimated_cost)))
        .unwrap_or_else(|| meta.estimated_cost.score())
}

fn predicted_ms_to_key(predicted_ms: f64) -> u64 {
    (predicted_ms.max(0.0) * 1_000.0)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

impl ObservedCostModel {
    fn is_informative(&self) -> bool {
        self.intercept_ms > 0.0 || self.coefficients_ms.iter().any(|&value| value > 0.0)
    }

    fn predict_ms(self, cost: ChunkEstimatedCost) -> f64 {
        let features = observed_cost_features(cost);
        let log_prediction = self.intercept_ms
            + self
                .coefficients_ms
                .into_iter()
                .zip(features)
                .map(|(weight, value)| weight * value)
                .sum::<f64>();
        self.clamp_prediction(log_prediction.exp_m1().max(0.0))
    }

    fn clamp_prediction(self, predicted_ms: f64) -> f64 {
        let lower = self.observed_min_ms.min(self.observed_median_ms).max(0.0) * 0.5;
        let upper = self
            .observed_max_ms
            .max(self.observed_p90_ms * 2.0)
            .max(self.observed_median_ms * 2.5)
            .max(lower + 1.0);
        predicted_ms.clamp(lower, upper)
    }
}

fn observed_cost_features(cost: ChunkEstimatedCost) -> [f64; OBSERVED_ADAPTIVE_FEATURE_COUNT] {
    [
        cost.used_records as f64 / 1024.0,
        cost.usage_transitions as f64 / 32.0,
        cost.sparse_segments as f64,
        cost.discontinuities as f64,
        cost.overlapped_segments as f64,
        cost.physical_span_bytes as f64 / (1024.0 * 1024.0),
        cost.covered_bytes as f64 / (1024.0 * 1024.0),
    ]
}

fn fit_observed_cost_model(samples: &[ObservedChunkSample]) -> ObservedCostModel {
    if samples.is_empty() {
        return ObservedCostModel::default();
    }

    let sample_stats = observed_sample_stats(samples);

    let dimensions = OBSERVED_ADAPTIVE_FEATURE_COUNT + 1;
    let mut normal = vec![vec![0.0_f64; dimensions + 1]; dimensions];
    for sample in samples {
        let mut row = [0.0_f64; OBSERVED_ADAPTIVE_FEATURE_COUNT + 1];
        row[0] = 1.0;
        row[1..].copy_from_slice(&sample.features);
        let target = sample.elapsed_ms.max(0.0).ln_1p();
        for i in 0..dimensions {
            for j in 0..dimensions {
                normal[i][j] += row[i] * row[j];
            }
            normal[i][dimensions] += row[i] * target;
        }
    }

    for (index, row) in normal.iter_mut().enumerate() {
        row[index] += OBSERVED_ADAPTIVE_RIDGE_LAMBDA;
    }

    let Some(solution) = solve_linear_system(normal) else {
        return ObservedCostModel::default();
    };

    let intercept_ms = solution[0].max(0.0);
    let mut coefficients_ms = [0.0; OBSERVED_ADAPTIVE_FEATURE_COUNT];
    for (slot, value) in coefficients_ms.iter_mut().zip(solution.iter().copied().skip(1)) {
        *slot = value.max(0.0);
    }
    ObservedCostModel {
        intercept_ms,
        coefficients_ms,
        observed_min_ms: sample_stats.min_ms,
        observed_median_ms: sample_stats.median_ms,
        observed_p90_ms: sample_stats.p90_ms,
        observed_max_ms: sample_stats.max_ms,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ObservedSampleStats {
    min_ms: f64,
    median_ms: f64,
    p90_ms: f64,
    max_ms: f64,
}

fn observed_sample_stats(samples: &[ObservedChunkSample]) -> ObservedSampleStats {
    let mut values = samples.iter().map(|sample| sample.elapsed_ms).collect::<Vec<_>>();
    values.sort_unstable_by(f64::total_cmp);
    let len = values.len();
    let percentile_index = |numerator: usize, denominator: usize| -> usize {
        if len <= 1 {
            0
        } else {
            ((len - 1) * numerator + denominator / 2) / denominator
        }
    };

    ObservedSampleStats {
        min_ms: *values.first().unwrap_or(&0.0),
        median_ms: values[percentile_index(1, 2)],
        p90_ms: values[percentile_index(9, 10)],
        max_ms: *values.last().unwrap_or(&0.0),
    }
}

fn solve_linear_system(mut augmented: Vec<Vec<f64>>) -> Option<Vec<f64>> {
    let n = augmented.len();
    for pivot in 0..n {
        let best_row = (pivot..n).max_by(|&left, &right| {
            augmented[left][pivot]
                .abs()
                .total_cmp(&augmented[right][pivot].abs())
        })?;
        if augmented[best_row][pivot].abs() <= f64::EPSILON {
            return None;
        }
        if best_row != pivot {
            augmented.swap(best_row, pivot);
        }

        let pivot_value = augmented[pivot][pivot];
        for column in pivot..=n {
            augmented[pivot][column] /= pivot_value;
        }

        for row in 0..n {
            if row == pivot {
                continue;
            }
            let factor = augmented[row][pivot];
            if factor.abs() <= f64::EPSILON {
                continue;
            }
            for column in pivot..=n {
                augmented[row][column] -= factor * augmented[pivot][column];
            }
        }
    }

    Some(augmented.into_iter().map(|row| row[n]).collect())
}

fn cost_aware_banded_execution_order(
    chunks: &[RawMftWorkChunk],
    extent_map: &ExtentMap,
    bitmap: &[u8],
    chunk_costs: &[ChunkEstimatedCost],
    worker_count: usize,
) -> Box<[usize]> {
    let mut execution = (0..chunks.len())
        .map(|index| ChunkExecutionMeta {
            index,
            physical_order_key: chunk_physical_order_key(chunks[index], extent_map),
            estimated_cost: chunk_costs[index],
            claim_order: 0,
            band_index: 0,
            band_position: 0,
            prediction_source: schedule_profile::PredictionSource::StaticEstimate,
            predicted_order_key: chunk_costs[index].score(),
        })
        .collect::<Vec<_>>();
    let _ = bitmap;
    execution.sort_unstable_by_key(|meta| meta.physical_order_key);

    let band_size = cost_aware_band_size(worker_count, execution.len());
    let wave_size = worker_count.max(1);
    let mut ordered = Vec::with_capacity(execution.len());

    for band in execution.chunks_mut(band_size) {
        band.sort_unstable_by(|left, right| {
            right
                .estimated_cost
                .score()
                .cmp(&left.estimated_cost.score())
                .then_with(|| left.physical_order_key.cmp(&right.physical_order_key))
        });
        for wave in band.chunks_mut(wave_size) {
            wave.sort_unstable_by_key(|meta| meta.physical_order_key);
        }
        ordered.extend(band.iter().map(|meta| meta.index));
    }

    ordered.into_boxed_slice()
}

fn cost_aware_band_size(worker_count: usize, chunk_count: usize) -> usize {
    worker_count
        .max(1)
        .saturating_mul(COST_AWARE_BAND_WAVES)
        .max(MIN_COST_AWARE_BAND_CHUNKS)
        .min(chunk_count.max(1))
}

fn estimate_chunk_costs(
    chunks: &[RawMftWorkChunk],
    extent_map: &ExtentMap,
    bitmap: &[u8],
    attr_list_hints: Option<&[ChunkAttrListHintSummary]>,
) -> Box<[ChunkEstimatedCost]> {
    chunks
        .iter()
        .enumerate()
        .map(|(index, &chunk)| {
            estimate_chunk_cost(
                chunk,
                extent_map,
                bitmap,
                attr_list_hints.and_then(|hints| hints.get(index).copied()),
            )
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn estimate_chunk_cost(
    chunk: RawMftWorkChunk,
    extent_map: &ExtentMap,
    bitmap: &[u8],
    attr_list_hint: Option<ChunkAttrListHintSummary>,
) -> ChunkEstimatedCost {
    let covered_bytes = chunk.record_len().saturating_mul(extent_map.file_record_size);
    let (used_records, usage_transitions) = bitmap_chunk_cost(chunk, bitmap);
    let attr_list_hint = attr_list_hint.unwrap_or_default();
    let chunk_start = chunk.start_record.saturating_mul(extent_map.file_record_size);
    let chunk_end = chunk.end_record.saturating_mul(extent_map.file_record_size);
    if chunk_start >= chunk_end || extent_map.cluster_size == 0 {
        return ChunkEstimatedCost {
            used_records,
            usage_transitions,
            attr_list_records: attr_list_hint.attr_list_records,
            nonresident_attr_lists: attr_list_hint.nonresident_attr_lists,
            enrich_candidates: attr_list_hint.enrich_candidates,
            referenced_extension_records: attr_list_hint.referenced_extension_records,
            sparse_segments: 0,
            discontinuities: 0,
            overlapped_segments: 0,
            physical_span_bytes: 0,
            covered_bytes,
        };
    }

    let mut sparse_segments = 0usize;
    let mut overlapped_segments = 0usize;
    let mut discontinuities = 0usize;
    let mut first_physical = None;
    let mut last_physical_end = None;
    let mut previous_data_end = None;

    for segment in &extent_map.segments {
        let segment_start = segment.vcn_start.saturating_mul(extent_map.cluster_size);
        let segment_end = segment_start
            .saturating_add(segment.clusters.saturating_mul(extent_map.cluster_size));
        if segment_end <= chunk_start {
            continue;
        }
        if segment_start >= chunk_end {
            break;
        }

        overlapped_segments += 1;
        let overlap_start = segment_start.max(chunk_start);
        let overlap_end = segment_end.min(chunk_end);
        if overlap_start >= overlap_end {
            continue;
        }

        let Some(lcn) = segment.lcn else {
            sparse_segments += 1;
            previous_data_end = None;
            continue;
        };

        let physical_segment_start = lcn.saturating_mul(extent_map.cluster_size);
        let physical_start = physical_segment_start.saturating_add(overlap_start - segment_start);
        let physical_end = physical_start.saturating_add(overlap_end - overlap_start);

        if let Some(previous_end) = previous_data_end
            && previous_end != physical_start
        {
            discontinuities += 1;
        }

        first_physical.get_or_insert(physical_start);
        last_physical_end = Some(physical_end);
        previous_data_end = Some(physical_end);
    }

    ChunkEstimatedCost {
        used_records,
        usage_transitions,
        attr_list_records: attr_list_hint.attr_list_records,
        nonresident_attr_lists: attr_list_hint.nonresident_attr_lists,
        enrich_candidates: attr_list_hint.enrich_candidates,
        referenced_extension_records: attr_list_hint.referenced_extension_records,
        sparse_segments: narrow_u16(sparse_segments),
        discontinuities: narrow_u16(discontinuities),
        overlapped_segments: narrow_u16(overlapped_segments),
        physical_span_bytes: match (first_physical, last_physical_end) {
            (Some(start), Some(end)) if end >= start => end - start,
            _ => 0,
        },
        covered_bytes,
    }
}

fn sample_chunk_attr_list_hints(
    mft: &RawMft<'_>,
    chunks: &[RawMftWorkChunk],
    options: &RawMftScanOptions,
) -> Result<Box<[ChunkAttrListHintSummary]>, UsnError> {
    let (mut reader, mut attr_reader) = mft.buffered_readers_for_options(options)?;
    let mut hints = Vec::with_capacity(chunks.len());
    for &chunk in chunks {
        let mut hint = ChunkAttrListHintSummary::default();
        for record_number in chunk.start_record..chunk.end_record {
            if hint.sampled_records as usize >= COST_HINT_SAMPLE_RECORDS {
                break;
            }
            if options.skip_unused() && !mft.bitmap_used(record_number) {
                continue;
            }
            let Some((entry, attr_list)) = read_batch_record_raw(
                &mut reader,
                &mft.boot,
                mft.extent_map.as_ref(),
                record_number,
                options.entry.collect_dos_file_name_links,
            )?
            else {
                continue;
            };
            if options.skip_extension_records() && entry.entry.base_record_reference != 0 {
                continue;
            }
            hint.sampled_records = hint.sampled_records.saturating_add(1);
            if let Some(attr_list) = attr_list {
                let record_hint = estimate_batch_attr_list_hint(
                    &entry,
                    attr_list,
                    record_number,
                    &mut attr_reader,
                    &mft.boot,
                    options.attr_list_batch_mode(),
                );
                hint = merge_chunk_attr_list_hint(hint, record_hint);
            }
        }
        hints.push(hint);
    }
    Ok(hints.into_boxed_slice())
}

fn merge_chunk_attr_list_hint(
    mut summary: ChunkAttrListHintSummary,
    record_hint: BatchAttrListHint,
) -> ChunkAttrListHintSummary {
    if record_hint.attr_list_present {
        summary.attr_list_records = summary.attr_list_records.saturating_add(1);
    }
    if record_hint.nonresident_attr_list {
        summary.nonresident_attr_lists = summary.nonresident_attr_lists.saturating_add(1);
    }
    if record_hint.needs_enrich {
        summary.enrich_candidates = summary.enrich_candidates.saturating_add(1);
    }
    summary.referenced_extension_records = summary
        .referenced_extension_records
        .saturating_add(record_hint.referenced_extension_records);
    summary
}

fn narrow_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn bitmap_chunk_cost(chunk: RawMftWorkChunk, bitmap: &[u8]) -> (u16, u16) {
    if bitmap.is_empty() {
        return (narrow_u16(chunk.record_len() as usize), 0);
    }

    let mut used_records = 0usize;
    let mut usage_transitions = 0usize;
    let mut previous = None;
    let end_record = chunk.end_record.min(chunk.start_record.saturating_add(u16::MAX as u64));
    for record_number in chunk.start_record..end_record {
        let used = bitmap_record_is_used(bitmap, record_number);
        used_records += usize::from(used);
        if let Some(previous_used) = previous
            && previous_used != used
        {
            usage_transitions += 1;
        }
        previous = Some(used);
    }
    (narrow_u16(used_records), narrow_u16(usage_transitions))
}

fn bitmap_record_is_used(bitmap: &[u8], record_number: u64) -> bool {
    let byte_index = (record_number / 8) as usize;
    let bit_index = (record_number % 8) as u8;
    bitmap
        .get(byte_index)
        .map(|byte| (byte >> bit_index) & 1 != 0)
        .unwrap_or(false)
}

fn attr_list_sampling_cost_hints_enabled() -> bool {
    std::env::var_os("USN_RAW_MFT_BENCH_COST_HINT_ATTR_SAMPLE").is_some()
}

fn chunk_physical_order_key(chunk: RawMftWorkChunk, extent_map: &ExtentMap) -> (u64, u64, u64) {
    match extent_map.record_offset(chunk.start_record) {
        Ok(Some(offset)) => (0, offset, chunk.start_record),
        Ok(None) => (1, u64::MAX - 1, chunk.start_record),
        Err(_) => (2, u64::MAX, chunk.start_record),
    }
}

fn chunk_physical_start_offset(chunk: RawMftWorkChunk, extent_map: &ExtentMap) -> Option<u64> {
    extent_map.record_offset(chunk.start_record).ok().flatten()
}

/// Return the half-open chunk-index range assigned to one worker under contiguous scheduling.
fn contiguous_worker_range(
    chunk_count: usize,
    worker_count: usize,
    worker_index: usize,
) -> (usize, usize) {
    let base = chunk_count / worker_count;
    let extra = chunk_count % worker_count;
    let start = worker_index * base + worker_index.min(extra);
    let len = base + usize::from(worker_index < extra);
    (start, start + len)
}

/// Build a stable error when available parallelism cannot be queried.
pub(crate) fn available_parallelism_error(error: io::Error) -> UsnError {
    UsnError::Io(io::Error::other(format!(
        "failed to query available parallelism: {error}"
    )))
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

#[cfg(test)]
mod tests {
    use super::{
        ChunkEstimatedCost, ChunkExecutionMeta, ChunkScheduling, ObservedChunkSample,
        adaptive_min_samples, contiguous_worker_range, cost_aware_band_size,
        dynamic_execution_order, estimate_chunk_costs, fit_observed_cost_model,
        observed_cost_features,
        order_band_for_execution,
    };
    use crate::raw_mft::schedule_profile::PredictionSource;
    use crate::raw_mft::{
        RawMftWorkChunk,
        layout::{data_run::DataRun, extent::ExtentMap},
    };

    fn test_extent_map(runs: &[DataRun]) -> ExtentMap {
        ExtentMap::from_runs(runs, 4096, 1024)
    }

    #[test]
    fn physical_order_sort_uses_chunk_start_offsets() {
        let extent_map = test_extent_map(&[
            DataRun::Data {
                lcn: 200,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 100,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 300,
                clusters: 1,
            },
        ]);
        let chunks = vec![
            RawMftWorkChunk {
                start_record: 0,
                end_record: 4,
            },
            RawMftWorkChunk {
                start_record: 4,
                end_record: 8,
            },
            RawMftWorkChunk {
                start_record: 8,
                end_record: 12,
            },
        ];

        let order = dynamic_execution_order(
            &chunks,
            &extent_map,
            &[],
            &estimate_chunk_costs(&chunks, &extent_map, &[], None),
            ChunkScheduling::DynamicPhysicalOrder,
            2,
        );

        assert_eq!(&*order, &[1, 0, 2]);
    }

    #[test]
    fn cost_banded_order_front_loads_fragmented_chunks_inside_a_band() {
        let extent_map = test_extent_map(&[
            DataRun::Data {
                lcn: 10,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 50,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 11,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 51,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 12,
                clusters: 1,
            },
            DataRun::Data {
                lcn: 52,
                clusters: 1,
            },
        ]);
        let chunks = vec![
            RawMftWorkChunk {
                start_record: 0,
                end_record: 8,
            },
            RawMftWorkChunk {
                start_record: 8,
                end_record: 12,
            },
            RawMftWorkChunk {
                start_record: 12,
                end_record: 20,
            },
            RawMftWorkChunk {
                start_record: 20,
                end_record: 24,
            },
        ];

        let order = dynamic_execution_order(
            &chunks,
            &extent_map,
            &[],
            &estimate_chunk_costs(&chunks, &extent_map, &[], None),
            ChunkScheduling::DynamicCostBanded,
            2,
        );

        assert_eq!(&*order, &[0, 2, 1, 3]);
    }

    #[test]
    fn cost_aware_band_size_scales_with_worker_count() {
        assert_eq!(cost_aware_band_size(1, 3), 3);
        assert_eq!(cost_aware_band_size(2, 16), 8);
        assert_eq!(cost_aware_band_size(11, 100), 44);
    }

    #[test]
    fn observed_cost_model_front_loads_features_that_measured_slow() {
        let slow = ChunkEstimatedCost {
            discontinuities: 4,
            physical_span_bytes: 64 * 1024 * 1024,
            covered_bytes: 8 * 1024,
            ..ChunkEstimatedCost::default()
        };
        let fast = ChunkEstimatedCost {
            covered_bytes: 8 * 1024,
            ..ChunkEstimatedCost::default()
        };
        let model = fit_observed_cost_model(&[
            ObservedChunkSample {
                features: observed_cost_features(slow),
                elapsed_ms: 40.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(slow),
                elapsed_ms: 42.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast),
                elapsed_ms: 8.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast),
                elapsed_ms: 9.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast),
                elapsed_ms: 10.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(slow),
                elapsed_ms: 39.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast),
                elapsed_ms: 11.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(slow),
                elapsed_ms: 41.0,
            },
        ]);

        assert!(model.predict_ms(slow) > model.predict_ms(fast));
    }

    #[test]
    fn observed_cost_model_clamps_prediction_spikes_to_observed_range() {
        let typical = ChunkEstimatedCost {
            covered_bytes: 8 * 1024,
            physical_span_bytes: 2 * 1024 * 1024,
            ..ChunkEstimatedCost::default()
        };
        let extreme = ChunkEstimatedCost {
            discontinuities: 64,
            sparse_segments: 32,
            overlapped_segments: 32,
            physical_span_bytes: 512 * 1024 * 1024,
            covered_bytes: 8 * 1024,
            ..ChunkEstimatedCost::default()
        };
        let model = fit_observed_cost_model(&[
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 24.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 28.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 31.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 36.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 40.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 44.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 48.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(typical),
                elapsed_ms: 52.0,
            },
        ]);

        assert!(model.predict_ms(extreme) <= 104.0);
        assert!(model.predict_ms(extreme) >= 12.0);
    }

    #[test]
    fn adaptive_min_samples_waits_for_about_one_band_of_samples() {
        assert_eq!(adaptive_min_samples(4, 16), 16);
        assert_eq!(adaptive_min_samples(11, 44), 44);
        assert_eq!(adaptive_min_samples(1, 3), 3);
    }

    #[test]
    fn adaptive_band_order_uses_observed_model_before_physical_tiebreak() {
        let fast = ChunkExecutionMeta {
            index: 0,
            physical_order_key: (0, 10, 0),
            claim_order: 0,
            band_index: 0,
            band_position: 0,
            prediction_source: PredictionSource::StaticEstimate,
            predicted_order_key: 0,
            estimated_cost: ChunkEstimatedCost {
                covered_bytes: 8 * 1024,
                ..ChunkEstimatedCost::default()
            },
        };
        let slow = ChunkExecutionMeta {
            index: 1,
            physical_order_key: (0, 20, 1),
            claim_order: 0,
            band_index: 0,
            band_position: 0,
            prediction_source: PredictionSource::StaticEstimate,
            predicted_order_key: 0,
            estimated_cost: ChunkEstimatedCost {
                discontinuities: 3,
                physical_span_bytes: 48 * 1024 * 1024,
                covered_bytes: 8 * 1024,
                ..ChunkEstimatedCost::default()
            },
        };
        let model = fit_observed_cost_model(&[
            ObservedChunkSample {
                features: observed_cost_features(slow.estimated_cost),
                elapsed_ms: 45.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(slow.estimated_cost),
                elapsed_ms: 44.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(slow.estimated_cost),
                elapsed_ms: 46.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast.estimated_cost),
                elapsed_ms: 12.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast.estimated_cost),
                elapsed_ms: 11.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast.estimated_cost),
                elapsed_ms: 10.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(slow.estimated_cost),
                elapsed_ms: 43.0,
            },
            ObservedChunkSample {
                features: observed_cost_features(fast.estimated_cost),
                elapsed_ms: 9.0,
            },
        ]);
        let mut band = vec![fast, slow];

        order_band_for_execution(&mut band, 1, Some(model));

        assert_eq!(band[0].index, 1);
        assert_eq!(band[1].index, 0);
    }

    #[test]
    fn contiguous_ranges_cover_all_chunks() {
        assert_eq!(contiguous_worker_range(10, 3, 0), (0, 4));
        assert_eq!(contiguous_worker_range(10, 3, 1), (4, 7));
        assert_eq!(contiguous_worker_range(10, 3, 2), (7, 10));
    }
}

