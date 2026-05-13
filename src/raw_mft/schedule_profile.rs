use std::{cmp::Reverse, sync::atomic::{AtomicBool, Ordering}, sync::{Mutex, OnceLock}, time::Duration};

use super::RawMftWorkChunk;

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: OnceLock<Mutex<ScheduleProfileState>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PredictionSource {
    #[default]
    StaticEstimate,
    ObservedModel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ChunkCostProfile {
    pub(crate) used_records: u16,
    pub(crate) usage_transitions: u16,
    pub(crate) attr_list_records: u16,
    pub(crate) nonresident_attr_lists: u16,
    pub(crate) enrich_candidates: u16,
    pub(crate) referenced_extension_records: u16,
    pub(crate) sparse_segments: u16,
    pub(crate) discontinuities: u16,
    pub(crate) overlapped_segments: u16,
    pub(crate) physical_span_bytes: u64,
    pub(crate) covered_bytes: u64,
}

impl ChunkCostProfile {
    #[must_use]
    pub(crate) fn score(self) -> u64 {
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WorkerScheduleSnapshot {
    pub(crate) worker_index: usize,
    pub(crate) chunks: u64,
    pub(crate) records: u64,
    pub(crate) covered_bytes: u64,
    pub(crate) total_elapsed: Duration,
    pub(crate) max_chunk_elapsed: Duration,
    pub(crate) total_estimated_cost: u64,
    pub(crate) max_estimated_cost: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkScheduleSnapshot {
    pub(crate) worker_index: usize,
    pub(crate) chunk_index: usize,
    pub(crate) chunk: RawMftWorkChunk,
    pub(crate) elapsed: Duration,
    pub(crate) estimated_cost: ChunkCostProfile,
    pub(crate) claim_order: usize,
    pub(crate) band_index: Option<usize>,
    pub(crate) band_position: Option<usize>,
    pub(crate) prediction_source: PredictionSource,
    pub(crate) predicted_order_key: u64,
    pub(crate) predicted_rank: usize,
    pub(crate) physical_start_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BandDecisionSnapshot {
    pub(crate) band_index: usize,
    pub(crate) chunk_count: usize,
    pub(crate) sample_count_before: usize,
    pub(crate) prediction_source: PredictionSource,
    pub(crate) front_chunk_index: usize,
    pub(crate) front_prediction_key: u64,
    pub(crate) back_chunk_index: usize,
    pub(crate) back_prediction_key: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScheduleProfileSnapshot {
    pub(crate) workers: Vec<WorkerScheduleSnapshot>,
    pub(crate) slowest_chunks: Vec<ChunkScheduleSnapshot>,
    pub(crate) band_decisions: Vec<BandDecisionSnapshot>,
    pub(crate) compared_top_k: usize,
    pub(crate) predicted_actual_top_overlap: usize,
    pub(crate) actual_top_in_predicted_top_half: usize,
    pub(crate) actual_top_in_predicted_top_quarter: usize,
    pub(crate) actual_top_missed_by_predicted_top_k: usize,
    pub(crate) predicted_top_false_positives: usize,
    pub(crate) actual_top_worst_predicted_rank: usize,
}

#[derive(Debug, Default)]
struct WorkerScheduleState {
    chunks: u64,
    records: u64,
    covered_bytes: u64,
    total_elapsed_nanos: u128,
    max_chunk_elapsed_nanos: u128,
    total_estimated_cost: u64,
    max_estimated_cost: u64,
}

#[derive(Debug, Default)]
struct ScheduleProfileState {
    workers: Vec<WorkerScheduleState>,
    chunks: Vec<ChunkScheduleSnapshot>,
    band_decisions: Vec<BandDecisionSnapshot>,
}

#[must_use]
pub(crate) struct ScheduleProfileGuard;

impl Drop for ScheduleProfileGuard {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Release);
    }
}

pub(crate) fn start() -> ScheduleProfileGuard {
    reset();
    ENABLED.store(true, Ordering::Release);
    ScheduleProfileGuard
}

pub(crate) fn is_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub(crate) fn ensure_worker_slot(worker_index: usize) {
    if !is_enabled() {
        return;
    }
    if let Ok(mut state) = profile_state().lock() {
        if state.workers.len() <= worker_index {
            state
                .workers
                .resize_with(worker_index + 1, WorkerScheduleState::default);
        }
    }
}

pub(crate) fn record_chunk_completion(
    worker_index: usize,
    chunk_index: usize,
    chunk: RawMftWorkChunk,
    estimated_cost: ChunkCostProfile,
    claim_order: usize,
    band_index: Option<usize>,
    band_position: Option<usize>,
    prediction_source: PredictionSource,
    predicted_order_key: u64,
    physical_start_offset: Option<u64>,
    elapsed: Duration,
) {
    if !is_enabled() {
        return;
    }

    let Ok(mut state) = profile_state().lock() else {
        return;
    };
    if state.workers.len() <= worker_index {
        state
            .workers
            .resize_with(worker_index + 1, WorkerScheduleState::default);
    }
    let worker = &mut state.workers[worker_index];
    let elapsed_nanos = elapsed.as_nanos();
    let estimated_score = estimated_cost.score();
    worker.chunks = worker.chunks.saturating_add(1);
    worker.records = worker.records.saturating_add(chunk.record_len());
    worker.covered_bytes = worker.covered_bytes.saturating_add(estimated_cost.covered_bytes);
    worker.total_elapsed_nanos = worker.total_elapsed_nanos.saturating_add(elapsed_nanos);
    worker.max_chunk_elapsed_nanos = worker.max_chunk_elapsed_nanos.max(elapsed_nanos);
    worker.total_estimated_cost = worker.total_estimated_cost.saturating_add(estimated_score);
    worker.max_estimated_cost = worker.max_estimated_cost.max(estimated_score);
    state.chunks.push(ChunkScheduleSnapshot {
        worker_index,
        chunk_index,
        chunk,
        elapsed,
        estimated_cost,
        claim_order,
        band_index,
        band_position,
        prediction_source,
        predicted_order_key,
        predicted_rank: 0,
        physical_start_offset,
    });
}

pub(crate) fn record_band_decision(
    band_index: usize,
    chunk_count: usize,
    sample_count_before: usize,
    prediction_source: PredictionSource,
    front_chunk_index: usize,
    front_prediction_key: u64,
    back_chunk_index: usize,
    back_prediction_key: u64,
) {
    if !is_enabled() {
        return;
    }

    let Ok(mut state) = profile_state().lock() else {
        return;
    };
    state.band_decisions.push(BandDecisionSnapshot {
        band_index,
        chunk_count,
        sample_count_before,
        prediction_source,
        front_chunk_index,
        front_prediction_key,
        back_chunk_index,
        back_prediction_key,
    });
}

pub(crate) fn snapshot() -> ScheduleProfileSnapshot {
    let Ok(state) = profile_state().lock() else {
        return ScheduleProfileSnapshot::default();
    };

    let workers = state
        .workers
        .iter()
        .enumerate()
        .map(|(worker_index, worker)| WorkerScheduleSnapshot {
            worker_index,
            chunks: worker.chunks,
            records: worker.records,
            covered_bytes: worker.covered_bytes,
            total_elapsed: duration_from_nanos(worker.total_elapsed_nanos),
            max_chunk_elapsed: duration_from_nanos(worker.max_chunk_elapsed_nanos),
            total_estimated_cost: worker.total_estimated_cost,
            max_estimated_cost: worker.max_estimated_cost,
        })
        .collect::<Vec<_>>();

    let mut predicted_order = state.chunks.clone();
    predicted_order.sort_unstable_by(|left, right| {
        right
            .predicted_order_key
            .cmp(&left.predicted_order_key)
            .then_with(|| left.chunk_index.cmp(&right.chunk_index))
    });
    let max_chunk_index = state
        .chunks
        .iter()
        .map(|chunk| chunk.chunk_index)
        .max()
        .map(|value| value + 1)
        .unwrap_or(0);
    let mut predicted_ranks = vec![0usize; max_chunk_index];
    for (rank, chunk) in predicted_order.iter().enumerate() {
        if let Some(slot) = predicted_ranks.get_mut(chunk.chunk_index) {
            *slot = rank + 1;
        }
    }

    let top_k = predicted_order.len().min(8);
    let predicted_top = predicted_order
        .iter()
        .take(top_k)
        .map(|chunk| chunk.chunk_index)
        .collect::<Vec<_>>();

    let mut slowest_chunks = state.chunks.clone();
    slowest_chunks.sort_unstable_by_key(|chunk| {
        (
            Reverse(chunk.elapsed),
            Reverse(chunk.estimated_cost.score()),
            chunk.chunk_index,
        )
    });
    let actual_top = slowest_chunks
        .iter()
        .take(top_k)
        .map(|chunk| chunk.chunk_index)
        .collect::<Vec<_>>();
    let predicted_actual_top_overlap = actual_top
        .iter()
        .filter(|chunk_index| predicted_top.contains(chunk_index))
        .count();
    let predicted_top_half_cutoff = top_k.div_ceil(2).max(1);
    let predicted_top_quarter_cutoff = top_k.div_ceil(4).max(1);
    let actual_top_in_predicted_top_half = slowest_chunks
        .iter()
        .take(top_k)
        .filter(|chunk| {
            predicted_ranks
                .get(chunk.chunk_index)
                .copied()
                .unwrap_or(usize::MAX)
                <= predicted_top_half_cutoff
        })
        .count();
    let actual_top_in_predicted_top_quarter = slowest_chunks
        .iter()
        .take(top_k)
        .filter(|chunk| {
            predicted_ranks
                .get(chunk.chunk_index)
                .copied()
                .unwrap_or(usize::MAX)
                <= predicted_top_quarter_cutoff
        })
        .count();
    let actual_top_missed_by_predicted_top_k = top_k.saturating_sub(predicted_actual_top_overlap);
    let predicted_top_false_positives = top_k.saturating_sub(predicted_actual_top_overlap);
    let actual_top_worst_predicted_rank = slowest_chunks
        .iter()
        .take(top_k)
        .map(|chunk| predicted_ranks.get(chunk.chunk_index).copied().unwrap_or_default())
        .max()
        .unwrap_or_default();
    for chunk in &mut slowest_chunks {
        chunk.predicted_rank = predicted_ranks
            .get(chunk.chunk_index)
            .copied()
            .unwrap_or_default();
    }
    slowest_chunks.truncate(8);

    ScheduleProfileSnapshot {
        workers,
        slowest_chunks,
        band_decisions: state.band_decisions.clone(),
        compared_top_k: top_k,
        predicted_actual_top_overlap,
        actual_top_in_predicted_top_half,
        actual_top_in_predicted_top_quarter,
        actual_top_missed_by_predicted_top_k,
        predicted_top_false_positives,
        actual_top_worst_predicted_rank,
    }
}

fn reset() {
    if let Ok(mut state) = profile_state().lock() {
        *state = ScheduleProfileState::default();
    }
}

fn profile_state() -> &'static Mutex<ScheduleProfileState> {
    STATE.get_or_init(|| Mutex::new(ScheduleProfileState::default()))
}

fn duration_from_nanos(nanos: u128) -> Duration {
    Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkCostProfile, PredictionSource, RawMftWorkChunk, record_band_decision,
        record_chunk_completion, snapshot, start,
    };
    use std::time::Duration;

    #[test]
    fn snapshot_orders_slowest_chunks_first() {
        let _guard = start();
        record_chunk_completion(
            0,
            1,
            RawMftWorkChunk {
                start_record: 0,
                end_record: 4,
            },
            ChunkCostProfile {
                covered_bytes: 4096,
                ..ChunkCostProfile::default()
            },
            0,
            None,
            None,
            PredictionSource::StaticEstimate,
            10,
            Some(10),
            Duration::from_millis(10),
        );
        record_chunk_completion(
            1,
            2,
            RawMftWorkChunk {
                start_record: 4,
                end_record: 12,
            },
            ChunkCostProfile {
                discontinuities: 1,
                covered_bytes: 8192,
                ..ChunkCostProfile::default()
            },
            1,
            Some(0),
            Some(0),
            PredictionSource::ObservedModel,
            20,
            Some(20),
            Duration::from_millis(20),
        );
        record_band_decision(0, 2, 8, PredictionSource::ObservedModel, 2, 20, 1, 10);

        let snapshot = snapshot();
        assert_eq!(snapshot.workers.len(), 2);
        assert_eq!(snapshot.slowest_chunks.first().map(|chunk| chunk.chunk_index), Some(2));
        assert_eq!(snapshot.workers[1].records, 8);
        assert_eq!(snapshot.slowest_chunks.first().map(|chunk| chunk.predicted_rank), Some(1));
        assert_eq!(snapshot.band_decisions.len(), 1);
        assert_eq!(snapshot.predicted_actual_top_overlap, 2);
        assert_eq!(snapshot.actual_top_in_predicted_top_half, 1);
        assert_eq!(snapshot.actual_top_in_predicted_top_quarter, 1);
        assert_eq!(snapshot.actual_top_missed_by_predicted_top_k, 0);
        assert_eq!(snapshot.predicted_top_false_positives, 0);
        assert_eq!(snapshot.actual_top_worst_predicted_rank, 2);
    }
}








