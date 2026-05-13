//! Shared helpers for the raw-MFT parallel ingest benchmark and profiling example.

use std::{
    env,
    ffi::OsStr,
    num::{NonZeroU64, NonZeroUsize},
    sync::OnceLock,
    thread,
    time::Duration,
};

use rustc_hash::FxHashMap;

use crate::{Fid, errors::UsnError, volume::Volume};

use super::{
    FileNameNamespace, RawMft, RawMftBatchEntry, RawMftChunkPlanOptions, RawMftEntry, RawMftLink,
    RawMftScanOptions, RawMftWorkChunk, attr_list_profile, options::AttrListBatchMode,
    parallel::ChunkScheduling, schedule_profile,
};

/// Default main read buffer size for the parallel ingest path.
const DEFAULT_MAIN_BUFFER_BYTES: usize = 256 * 1024;
/// Default attribute-list read buffer size for the parallel ingest path.
const DEFAULT_ATTR_BUFFER_BYTES: usize = 16 * 1024;
/// Default number of logical records per chunk.
const DEFAULT_CHUNK_RECORDS: u64 = 2 * 1024;
/// First normal FILE record number in the NTFS `$MFT`.
const FIRST_NORMAL_RECORD: u64 = 24;

/// Environment-driven configuration for the ingest benchmark.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Drive letter used for the raw volume.
    pub drive: char,
    /// Number of worker threads used by the parallel path.
    pub worker_count: NonZeroUsize,
    /// Main sequential read buffer size in bytes.
    pub main_buffer_bytes: NonZeroUsize,
    /// Attribute-list read buffer size in bytes.
    pub attr_buffer_bytes: NonZeroUsize,
    /// Maximum logical records per work chunk.
    pub chunk_records: NonZeroU64,
    /// First logical record number to include.
    pub start_record: u64,
    /// Optional exclusive end record number.
    pub end_record: Option<u64>,
    /// Worker scheduling mode used by the parallel executor.
    scheduling: ChunkScheduling,
}

/// Benchmark-visible scheduling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchScheduling {
    Dynamic,
    DynamicPhysicalOrder,
    DynamicCostBanded,
    DynamicObservedAdaptive,
    Contiguous,
}

impl BenchScheduling {
    fn as_executor_mode(self) -> ChunkScheduling {
        match self {
            Self::Dynamic => ChunkScheduling::Dynamic,
            Self::DynamicPhysicalOrder => ChunkScheduling::DynamicPhysicalOrder,
            Self::DynamicCostBanded => ChunkScheduling::DynamicCostBanded,
            Self::DynamicObservedAdaptive => ChunkScheduling::DynamicObservedAdaptive,
            Self::Contiguous => ChunkScheduling::Contiguous,
        }
    }
}

impl BenchConfig {
    /// Build a benchmark configuration from environment variables and defaults.
    fn from_env() -> Self {
        Self {
            drive: pick_drive(),
            worker_count: parse_env_nonzero_usize(
                "USN_RAW_MFT_BENCH_WORKERS",
                default_worker_count(),
            ),
            main_buffer_bytes: parse_env_nonzero_usize(
                "USN_RAW_MFT_BENCH_BUFFER_BYTES",
                nonzero_usize(DEFAULT_MAIN_BUFFER_BYTES),
            ),
            attr_buffer_bytes: parse_env_nonzero_usize(
                "USN_RAW_MFT_BENCH_ATTR_BUFFER_BYTES",
                nonzero_usize(DEFAULT_ATTR_BUFFER_BYTES),
            ),
            chunk_records: parse_env_nonzero_u64(
                "USN_RAW_MFT_BENCH_CHUNK_RECORDS",
                nonzero_u64(DEFAULT_CHUNK_RECORDS),
            ),
            start_record: env::var("USN_RAW_MFT_BENCH_START_RECORD")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(FIRST_NORMAL_RECORD),
            end_record: env::var("USN_RAW_MFT_BENCH_END_RECORD")
                .ok()
                .and_then(|value| value.parse::<u64>().ok()),
            scheduling: parse_bench_scheduling(
                env::var("USN_RAW_MFT_BENCH_SCHEDULING").ok().as_deref(),
                BenchScheduling::Dynamic,
            )
            .as_executor_mode(),
        }
    }

    /// Build scan options for the serial ingest path.
    fn iter_options(&self) -> RawMftScanOptions {
        let builder = RawMftScanOptions::builder()
            .buffer_bytes(self.main_buffer_bytes)
            .attr_buffer_bytes(self.attr_buffer_bytes)
            .skip_unused(true)
            .skip_extension_records(true)
            .collect_alternate_data_streams(false)
            .collect_data_run_summary(false)
            .collect_dos_file_name_links(false)
            .start_record(self.start_record)
            .end_record(self.end_record);

        let builder = if summary_attr_list_light_enabled() {
            builder.attr_list_batch_mode(AttrListBatchMode::SummaryOnly)
        } else {
            builder
        };

        let builder = if sort_attr_list_extensions_by_offset_enabled() {
            builder.sort_attr_list_extensions_by_offset(true)
        } else {
            builder
        };

        let builder = if deferred_chunk_attr_list_enrichment_enabled() {
            builder.deferred_chunk_attr_list_enrichment(true)
        } else {
            builder.deferred_chunk_attr_list_enrichment(false)
        };

        let builder = if let Some(window_records) = deferred_chunk_attr_list_window_records() {
            builder.deferred_chunk_attr_list_window_records(window_records)
        } else {
            builder
        };

        builder.build()
    }

    /// Build chunk-planning options for the parallel ingest path.
    fn chunk_plan_options(&self) -> RawMftChunkPlanOptions {
        RawMftChunkPlanOptions::builder()
            .skip_unused(true)
            .start_record(self.start_record)
            .end_record(self.end_record)
            .max_records_per_chunk(self.chunk_records)
            .build()
    }
}

impl BenchConfig {
    /// Return a copy with a different worker count.
    #[must_use]
    pub fn with_worker_count(&self, worker_count: NonZeroUsize) -> Self {
        let mut config = self.clone();
        config.worker_count = worker_count;
        config
    }

    /// Return a copy with a different scheduling mode.
    #[must_use]
    pub fn with_scheduling(&self, scheduling: BenchScheduling) -> Self {
        let mut config = self.clone();
        config.scheduling = scheduling.as_executor_mode();
        config
    }

    /// Human-readable scheduling label for benchmark output.
    #[must_use]
    pub fn scheduling_label(&self) -> &'static str {
        match self.scheduling {
            ChunkScheduling::Dynamic => "dynamic",
            ChunkScheduling::DynamicPhysicalOrder => "dynamic-physical-order",
            ChunkScheduling::DynamicCostBanded => "dynamic-cost-banded",
            ChunkScheduling::DynamicObservedAdaptive => "dynamic-observed-adaptive",
            ChunkScheduling::Contiguous => "contiguous",
        }
    }

    /// Benchmark-visible scheduling mode for this config.
    #[must_use]
    pub fn scheduling_mode(&self) -> BenchScheduling {
        match self.scheduling {
            ChunkScheduling::Dynamic => BenchScheduling::Dynamic,
            ChunkScheduling::DynamicPhysicalOrder => BenchScheduling::DynamicPhysicalOrder,
            ChunkScheduling::DynamicCostBanded => BenchScheduling::DynamicCostBanded,
            ChunkScheduling::DynamicObservedAdaptive => BenchScheduling::DynamicObservedAdaptive,
            ChunkScheduling::Contiguous => BenchScheduling::Contiguous,
        }
    }
}

/// Compact metadata captured for one visible MFT node.
#[derive(Debug, Clone)]
struct BenchNodeMeta {
    /// Logical file size in bytes.
    size: u64,
    /// Allocated file size in bytes.
    allocated_size: u64,
}

/// Child link captured during the ingest walk.
#[derive(Debug, Clone)]
struct BenchChildLink {
    /// Record number of the child entry.
    child_record: u64,
}

/// Final summary returned by the ingest helpers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BenchSummary {
    /// Number of visible records captured.
    pub records: usize,
    /// Number of parent buckets in the child-link map.
    pub parent_buckets: usize,
    /// Total number of child links captured.
    pub child_links: usize,
    /// Sum of logical sizes across visible records.
    pub logical_bytes: u64,
    /// Sum of allocated sizes across visible records.
    pub allocated_bytes: u64,
}

/// Attr-list enrichment counters gathered during one exact-match ingest run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BenchAttrListProfile {
    /// Total records scanned by the chunk workers.
    pub records_scanned: u64,
    /// Records that carried a `$ATTRIBUTE_LIST`.
    pub records_with_attr_list: u64,
    /// Resident `$ATTRIBUTE_LIST` payload count.
    pub resident_attr_lists: u64,
    /// Non-resident `$ATTRIBUTE_LIST` payload count.
    pub nonresident_attr_lists: u64,
    /// Records whose parsed base metadata still required enrichment.
    pub records_needing_enrich: u64,
    /// Records that had an attribute list but were already complete enough to skip enrichment.
    pub records_skipped_after_need_check: u64,
    /// Number of enrichment calls performed.
    pub enrichment_calls: u64,
    /// Unique extension record references discovered across all enrichments.
    pub extension_records_referenced: u64,
    /// Extension records successfully loaded across all enrichments.
    pub extension_records_loaded: u64,
    /// Total resident `$ATTRIBUTE_LIST` payload bytes seen in base records.
    pub resident_attr_list_bytes: u64,
    /// Total logical bytes advertised by non-resident `$ATTRIBUTE_LIST` payloads.
    pub nonresident_attr_list_bytes: u64,
    /// Wall-clock time spent inside enrichment calls.
    pub enrich_wall_time: Duration,
    /// Time spent materializing resident/non-resident attr-list payload bytes.
    pub attr_list_materialize_time: Duration,
    /// Time spent scanning materialized attr-list payloads to collect extension references.
    pub extension_record_discovery_time: Duration,
    /// Number of attempted extension-record loads.
    pub extension_record_load_attempts: u64,
    /// Time spent loading and parsing extension records.
    pub extension_record_load_time: Duration,
    /// Time spent mapping an extension record number to a raw-disk byte offset.
    pub extension_offset_lookup_time: Duration,
    /// Time spent reading extension record bytes through the buffered volume reader.
    pub extension_record_read_time: Duration,
    /// Time spent validating and parsing extension record bytes into a `FileRecord`.
    pub extension_record_parse_time: Duration,
    /// Time spent building batch/rich entry state from a parsed extension record.
    pub extension_entry_build_time: Duration,
    /// Time spent visiting loaded extension records and merging them into the base entry.
    pub extension_record_visit_time: Duration,
    /// Number of file-name materializations performed while parsing extension records.
    pub extension_file_name_materialize_calls: u64,
    /// Total UTF-16 code units materialized into owned file names while parsing extension records.
    pub extension_file_name_code_units: u64,
    /// Time spent materializing extension-record file names.
    pub extension_file_name_materialize_time: Duration,
    /// Number of link-merge operations performed.
    pub link_merge_calls: u64,
    /// Total links produced by those merges.
    pub link_merge_output_links: u64,
    /// Count of inline-name copies performed while synthesizing merged links.
    pub link_inline_name_copies: u64,
    /// Count of pre-existing links copied into merged link vectors.
    pub link_slice_copy_inputs: u64,
    /// Time spent merging link sets.
    pub link_merge_time: Duration,
    /// Time spent merging data/reparse metadata from extension records.
    pub data_merge_time: Duration,
    /// Number of times a higher-scoring extension file name replaced the current selection.
    pub selected_name_replacements: u64,
    /// Time spent performing selected-name replacement assignments.
    pub selected_name_replace_time: Duration,
    /// Number of distinct extension records loaded during the run.
    pub unique_extension_records_loaded: u64,
    /// Number of distinct extension records that were loaded more than once.
    pub records_reloaded: u64,
    /// Total load attempts beyond the first load of each distinct extension record.
    pub repeated_extension_loads: u64,
    /// Largest number of loads observed for any single extension record.
    pub max_loads_for_single_extension_record: u64,
    /// Number of same-thread consecutive extension-load offset comparisons recorded.
    pub extension_offset_sequence_samples: u64,
    /// Same-thread consecutive extension loads whose offsets were exactly one FILE record apart.
    pub extension_offset_exact_adjacent: u64,
    /// Same-thread consecutive extension loads that moved forward on disk.
    pub extension_offset_forward: u64,
    /// Same-thread consecutive extension loads that moved backward on disk.
    pub extension_offset_backward: u64,
    /// Same-thread consecutive extension-load jumps of at most 1 MiB.
    pub extension_offset_jump_le_1_mib: u64,
    /// Same-thread consecutive extension-load jumps of at most 8 MiB.
    pub extension_offset_jump_le_8_mib: u64,
    /// Same-thread consecutive extension-load jumps greater than 64 MiB.
    pub extension_offset_jump_gt_64_mib: u64,
    /// Sum of absolute same-thread offset jumps across consecutive extension loads.
    pub extension_offset_abs_jump_bytes: u64,
    /// Maximum absolute same-thread offset jump seen across consecutive extension loads.
    pub extension_offset_max_abs_jump_bytes: u64,
    /// Number of extension loads with a known base-record offset for comparison.
    pub base_to_extension_samples: u64,
    /// Extension loads whose offset stayed within 1 MiB of their base record.
    pub base_to_extension_jump_le_1_mib: u64,
    /// Extension loads whose offset stayed within 8 MiB of their base record.
    pub base_to_extension_jump_le_8_mib: u64,
    /// Extension loads whose offset was more than 64 MiB away from their base record.
    pub base_to_extension_jump_gt_64_mib: u64,
    /// Sum of absolute base-to-extension offset distances.
    pub base_to_extension_abs_jump_bytes: u64,
    /// Maximum absolute base-to-extension offset distance seen.
    pub base_to_extension_max_abs_jump_bytes: u64,
    /// Extension loads that landed inside the current chunk's logical record range.
    pub extension_within_current_chunk: u64,
    /// Extension loads that landed outside the current chunk's logical record range.
    pub extension_outside_current_chunk: u64,
}

/// Aggregate per-worker and tail-chunk scheduling telemetry for one ingest run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BenchSchedulingProfile {
    /// One summary row per worker that completed at least one chunk.
    pub workers: Vec<BenchWorkerSchedulingProfile>,
    /// Slowest completed chunks in descending elapsed order.
    pub slowest_chunks: Vec<BenchSlowChunkProfile>,
    /// Per-band ordering decisions emitted by the observed-adaptive scheduler.
    pub band_decisions: Vec<BenchBandDecisionProfile>,
    /// Number of chunks compared between predicted and actual top lists.
    pub compared_top_k: usize,
    /// Overlap count between the predicted-heaviest and actual-slowest top chunk lists.
    pub predicted_actual_top_overlap: usize,
    /// Actual slowest top-K chunks that were at least ranked inside the predicted top half.
    pub actual_top_in_predicted_top_half: usize,
    /// Actual slowest top-K chunks that were at least ranked inside the predicted top quarter.
    pub actual_top_in_predicted_top_quarter: usize,
    /// Actual slowest top-K chunks that were missed by the predicted top-K entirely.
    pub actual_top_missed_by_predicted_top_k: usize,
    /// Predicted top-K chunks that did not end up in the actual slowest top-K.
    pub predicted_top_false_positives: usize,
    /// Worst predicted rank among the actual slowest top-K chunks.
    pub actual_top_worst_predicted_rank: usize,
}

/// Source of the ordering signal attached to one chunk or band decision.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BenchPredictionSource {
    #[default]
    StaticEstimate,
    ObservedModel,
}

/// One adaptive band-ordering decision taken before workers start claiming that band.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BenchBandDecisionProfile {
    pub band_index: usize,
    pub chunk_count: usize,
    pub sample_count_before: usize,
    pub prediction_source: BenchPredictionSource,
    pub front_chunk_index: usize,
    pub front_prediction_key: u64,
    pub back_chunk_index: usize,
    pub back_prediction_key: u64,
}

/// Worker-level scheduling telemetry for one ingest run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BenchWorkerSchedulingProfile {
    pub worker_index: usize,
    pub chunks: u64,
    pub records: u64,
    pub covered_bytes: u64,
    pub total_elapsed: Duration,
    pub max_chunk_elapsed: Duration,
    pub total_estimated_cost: u64,
    pub max_estimated_cost: u64,
}

/// Tail chunk telemetry for one completed chunk.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BenchSlowChunkProfile {
    pub worker_index: usize,
    pub chunk_index: usize,
    pub start_record: u64,
    pub end_record: u64,
    pub elapsed: Duration,
    pub claim_order: usize,
    pub band_index: Option<usize>,
    pub band_position: Option<usize>,
    pub prediction_source: BenchPredictionSource,
    pub predicted_order_key: u64,
    pub predicted_rank: usize,
    pub estimated_used_records: u16,
    pub estimated_usage_transitions: u16,
    pub estimated_attr_list_records: u16,
    pub estimated_nonresident_attr_lists: u16,
    pub estimated_enrich_candidates: u16,
    pub estimated_referenced_extension_records: u16,
    pub estimated_sparse_segments: u16,
    pub estimated_discontinuities: u16,
    pub estimated_overlapped_segments: u16,
    pub estimated_physical_span_bytes: u64,
    pub covered_bytes: u64,
    pub physical_start_offset: Option<u64>,
}

/// Optional exact-match profile outputs gathered during one ingest run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BenchRunProfiles {
    pub attr_list: Option<BenchAttrListProfile>,
    pub scheduling: Option<BenchSchedulingProfile>,
}

impl BenchAttrListProfile {
    /// Share of scanned records that carried a `$ATTRIBUTE_LIST`.
    #[must_use]
    pub fn attr_list_hit_rate(self) -> f64 {
        ratio(self.records_with_attr_list, self.records_scanned)
    }

    /// Share of attribute-list-bearing records that actually performed enrichment.
    #[must_use]
    pub fn enrich_rate_given_attr_list(self) -> f64 {
        ratio(self.records_needing_enrich, self.records_with_attr_list)
    }

    /// Mean referenced extension-record count per enrichment call.
    #[must_use]
    pub fn referenced_extensions_per_enrich(self) -> f64 {
        ratio(self.extension_records_referenced, self.enrichment_calls)
    }

    /// Mean successfully loaded extension-record count per enrichment call.
    #[must_use]
    pub fn loaded_extensions_per_enrich(self) -> f64 {
        ratio(self.extension_records_loaded, self.enrichment_calls)
    }

    /// Mean attempted extension-record load count per enrichment call.
    #[must_use]
    pub fn load_attempts_per_enrich(self) -> f64 {
        ratio(self.extension_record_load_attempts, self.enrichment_calls)
    }

    /// Mean links produced per link-merge operation.
    #[must_use]
    pub fn output_links_per_merge(self) -> f64 {
        ratio(self.link_merge_output_links, self.link_merge_calls)
    }

    /// Mean UTF-16 code-unit count per extension-record file name materialization.
    #[must_use]
    pub fn code_units_per_materialized_name(self) -> f64 {
        ratio(
            self.extension_file_name_code_units,
            self.extension_file_name_materialize_calls,
        )
    }

    /// Share of extension-load attempts that reloaded a previously seen extension record.
    #[must_use]
    pub fn repeated_extension_load_rate(self) -> f64 {
        ratio(
            self.repeated_extension_loads,
            self.extension_record_load_attempts,
        )
    }

    /// Share of same-thread consecutive extension loads that were exactly adjacent on disk.
    #[must_use]
    pub fn exact_adjacent_extension_load_rate(self) -> f64 {
        ratio(
            self.extension_offset_exact_adjacent,
            self.extension_offset_sequence_samples,
        )
    }

    /// Share of same-thread consecutive extension loads whose jump stayed within 1 MiB.
    #[must_use]
    pub fn extension_jump_le_1_mib_rate(self) -> f64 {
        ratio(
            self.extension_offset_jump_le_1_mib,
            self.extension_offset_sequence_samples,
        )
    }

    /// Share of same-thread consecutive extension loads whose jump stayed within 8 MiB.
    #[must_use]
    pub fn extension_jump_le_8_mib_rate(self) -> f64 {
        ratio(
            self.extension_offset_jump_le_8_mib,
            self.extension_offset_sequence_samples,
        )
    }

    /// Share of same-thread consecutive extension loads whose jump exceeded 64 MiB.
    #[must_use]
    pub fn extension_jump_gt_64_mib_rate(self) -> f64 {
        ratio(
            self.extension_offset_jump_gt_64_mib,
            self.extension_offset_sequence_samples,
        )
    }

    /// Mean absolute same-thread jump size between consecutive extension loads, in bytes.
    #[must_use]
    pub fn avg_extension_abs_jump_bytes(self) -> f64 {
        ratio(
            self.extension_offset_abs_jump_bytes,
            self.extension_offset_sequence_samples,
        )
    }

    /// Share of extension loads whose offsets stayed within 1 MiB of their base record.
    #[must_use]
    pub fn base_to_extension_le_1_mib_rate(self) -> f64 {
        ratio(
            self.base_to_extension_jump_le_1_mib,
            self.base_to_extension_samples,
        )
    }

    /// Share of extension loads whose offsets stayed within 8 MiB of their base record.
    #[must_use]
    pub fn base_to_extension_le_8_mib_rate(self) -> f64 {
        ratio(
            self.base_to_extension_jump_le_8_mib,
            self.base_to_extension_samples,
        )
    }

    /// Share of extension loads whose offsets were more than 64 MiB from their base record.
    #[must_use]
    pub fn base_to_extension_gt_64_mib_rate(self) -> f64 {
        ratio(
            self.base_to_extension_jump_gt_64_mib,
            self.base_to_extension_samples,
        )
    }

    /// Mean absolute base-to-extension offset distance, in bytes.
    #[must_use]
    pub fn avg_base_to_extension_abs_jump_bytes(self) -> f64 {
        ratio(
            self.base_to_extension_abs_jump_bytes,
            self.base_to_extension_samples,
        )
    }

    /// Share of extension loads that stayed inside the current chunk.
    #[must_use]
    pub fn within_current_chunk_rate(self) -> f64 {
        ratio(
            self.extension_within_current_chunk,
            self.extension_record_load_attempts,
        )
    }
}

impl BenchSchedulingProfile {
    #[must_use]
    pub fn max_worker_elapsed(&self) -> Duration {
        self.workers
            .iter()
            .map(|worker| worker.total_elapsed)
            .max()
            .unwrap_or(Duration::ZERO)
    }

    #[must_use]
    pub fn min_worker_elapsed(&self) -> Duration {
        self.workers
            .iter()
            .map(|worker| worker.total_elapsed)
            .min()
            .unwrap_or(Duration::ZERO)
    }

    #[must_use]
    pub fn observed_model_band_count(&self) -> usize {
        self.band_decisions
            .iter()
            .filter(|band| band.prediction_source == BenchPredictionSource::ObservedModel)
            .count()
    }

    #[must_use]
    pub fn static_estimate_band_count(&self) -> usize {
        self.band_decisions
            .iter()
            .filter(|band| band.prediction_source == BenchPredictionSource::StaticEstimate)
            .count()
    }

    #[must_use]
    pub fn actual_top_hit_rate(&self) -> f64 {
        ratio(
            self.predicted_actual_top_overlap as u64,
            self.compared_top_k as u64,
        )
    }

    #[must_use]
    pub fn actual_top_half_hit_rate(&self) -> f64 {
        ratio(
            self.actual_top_in_predicted_top_half as u64,
            self.compared_top_k as u64,
        )
    }

    #[must_use]
    pub fn actual_top_quarter_hit_rate(&self) -> f64 {
        ratio(
            self.actual_top_in_predicted_top_quarter as u64,
            self.compared_top_k as u64,
        )
    }
}

impl BenchPredictionSource {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::StaticEstimate => "static",
            Self::ObservedModel => "observed",
        }
    }
}

impl BenchWorkerSchedulingProfile {
    #[must_use]
    pub fn avg_chunk_elapsed(self) -> Duration {
        if self.chunks == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(self.total_elapsed.as_secs_f64() / self.chunks as f64)
        }
    }
}

impl BenchSlowChunkProfile {
    #[must_use]
    pub fn record_len(self) -> u64 {
        self.end_record.saturating_sub(self.start_record)
    }

    #[must_use]
    pub fn estimated_cost_score(self) -> u64 {
        let span_mib = self.estimated_physical_span_bytes / (1024 * 1024);
        (self.estimated_referenced_extension_records as u64) * 4_096
            + (self.estimated_enrich_candidates as u64) * 2_048
            + (self.estimated_attr_list_records as u64) * 1_024
            + (self.estimated_nonresident_attr_lists as u64) * 256
            + (self.estimated_used_records as u64) * 64
            + (self.estimated_usage_transitions as u64) * 32
            + (self.estimated_discontinuities as u64) * 16
            + (self.estimated_sparse_segments as u64) * 8
            + (self.estimated_overlapped_segments as u64) * 4
            + span_mib.min(4_096)
    }
}

/// Static description of the benchmarked workload shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchWorkloadShape {
    /// Total addressable file records in the `$MFT`.
    pub record_count: u64,
    /// Number of logical chunks planned for the current config.
    pub planned_chunks: usize,
    /// NTFS file-record size in bytes.
    pub file_record_size: u64,
    /// NTFS cluster size in bytes.
    pub cluster_size: u64,
}

/// Per-run mutable storage used while ingesting entries.
struct BenchTargets<'a> {
    /// Indexed table of visible records.
    records: &'a mut [Option<BenchNodeMeta>],
    /// Child links grouped by parent record number.
    children_by_parent: &'a mut FxHashMap<u64, Vec<BenchChildLink>>,
}

/// Worker-local accumulator used by the parallel folded path.
struct PartialIngest {
    /// Records collected for the current chunk.
    records: Vec<(u64, BenchNodeMeta)>,
    /// Child links collected for the current chunk.
    child_links: Vec<(u64, BenchChildLink)>,
}

/// Return the shared benchmark configuration singleton.
pub fn bench_config() -> &'static BenchConfig {
    static CONFIG: OnceLock<BenchConfig> = OnceLock::new();
    CONFIG.get_or_init(BenchConfig::from_env)
}

/// Print the benchmark configuration to stderr.
pub fn print_bench_config(config: &BenchConfig) {
    eprintln!(
        "raw_mft_ingest bench config: drive={} workers={} chunk_records={} main_buffer={} attr_buffer={} start_record={} end_record={}",
        config.drive,
        config.worker_count,
        config.chunk_records,
        config.main_buffer_bytes,
        config.attr_buffer_bytes,
        config.start_record,
        config
            .end_record
            .map(|value| value.to_string())
            .unwrap_or_else(|| "full".to_owned()),
    );
    eprintln!(
        "raw_mft_ingest bench scheduling: {}",
        config.scheduling_label()
    );
    if summary_attr_list_light_enabled() {
        eprintln!("raw_mft_ingest attr_list mode: summary-light");
    }
    if sort_attr_list_extensions_by_offset_enabled() {
        eprintln!("raw_mft_ingest attr_list order: offset-sorted");
    }
    if scheduling_profile_enabled() {
        eprintln!("raw_mft_ingest scheduling profile: enabled");
    }
    if cost_hint_attr_sampling_enabled() {
        match config.scheduling_mode() {
            BenchScheduling::DynamicCostBanded => {
                eprintln!("raw_mft_ingest cost hints: attr-list-sampled");
            }
            _ => {
                eprintln!(
                    "raw_mft_ingest cost hints: attr-list-sampled (inactive for current scheduling mode)"
                );
            }
        }
    }
    eprintln!(
        "raw_mft_ingest attr_list execution: {}",
        if deferred_chunk_attr_list_enrichment_enabled() {
            "deferred-chunk"
        } else {
            "legacy-per-record"
        }
    );
    if deferred_chunk_attr_list_enrichment_enabled()
        && let Some(window_records) = deferred_chunk_attr_list_window_records()
    {
        eprintln!(
            "raw_mft_ingest attr_list deferred window records: {}",
            window_records
        );
    }
}

/// Parse a comma-separated worker sweep list from the environment.
pub fn worker_sweep_values() -> Vec<NonZeroUsize> {
    parse_nonzero_usize_list("USN_RAW_MFT_BENCH_WORKERS_LIST")
}

/// Parse a comma-separated scheduling sweep list from the environment.
pub fn scheduling_sweep_values() -> Vec<BenchScheduling> {
    env::var("USN_RAW_MFT_BENCH_SCHEDULING_LIST")
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter_map(parse_bench_scheduling_token)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_default()
}

/// Return whether the serial ingest benchmark should also be run.
pub fn include_serial_bench() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_INCLUDE_SERIAL").is_some()
}

/// Return whether the benchmark harness should print an extra one-shot summary table.
pub fn print_summary_enabled() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_PRINT_SUMMARY").is_some()
}

/// Return whether the exact-match profile example should print attr-list counters.
pub fn attr_list_profile_enabled() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_PRINT_ATTR_LIST_PROFILE").is_some()
}

/// Return whether the exact-match profile example should print scheduling telemetry.
pub fn scheduling_profile_enabled() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_PRINT_SCHEDULING_PROFILE").is_some()
}

/// Return whether the experimental attr-list sampling cost model is enabled for `dynamic-cost-banded`.
pub fn cost_hint_attr_sampling_enabled() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_COST_HINT_ATTR_SAMPLE").is_some()
}

/// Return whether the ingest tooling should use the lighter summary-only attr-list mode.
pub fn summary_attr_list_light_enabled() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_SUMMARY_ATTR_LIST_LIGHT").is_some()
}

/// Return whether the ingest tooling should sort attr-list extension targets by offset before loading them.
pub fn sort_attr_list_extensions_by_offset_enabled() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_ATTR_LIST_SORT_BY_OFFSET").is_some()
}

/// Return whether chunk-parallel batch scans should defer attr-list extension loads until the whole chunk has been scanned.
pub fn deferred_chunk_attr_list_enrichment_enabled() -> bool {
    match env::var("USN_RAW_MFT_BENCH_DEFERRED_ATTR_LIST").ok() {
        None => false,
        Some(value) if value.eq_ignore_ascii_case("0") => false,
        Some(value) if value.eq_ignore_ascii_case("false") => false,
        Some(value) if value.eq_ignore_ascii_case("off") => false,
        Some(value) if value.eq_ignore_ascii_case("legacy") => false,
        Some(_) => true,
    }
}

/// Return the deferred attr-list flush window, measured in scanned base records.
pub fn deferred_chunk_attr_list_window_records() -> Option<usize> {
    env::var("USN_RAW_MFT_BENCH_DEFERRED_ATTR_LIST_WINDOW_RECORDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&value| value != 0)
}

/// Return how many one-shot runs should be used when printing the summary table.
pub fn summary_run_count() -> NonZeroUsize {
    parse_env_nonzero_usize("USN_RAW_MFT_BENCH_SUMMARY_RUNS", NonZeroUsize::MIN)
}

/// Describe the workload shape produced by the current benchmark config.
pub fn workload_shape(mft: &RawMft<'_>, config: &BenchConfig) -> BenchWorkloadShape {
    BenchWorkloadShape {
        record_count: mft.record_count(),
        planned_chunks: mft
            .plan_chunks_with_options(config.chunk_plan_options())
            .len(),
        file_record_size: mft.file_record_size(),
        cluster_size: mft.cluster_size(),
    }
}

/// Run the parallel ingest workload and return a compact summary.
pub fn run_parallel_ingest(
    mft: &RawMft<'_>,
    config: &BenchConfig,
) -> Result<BenchSummary, UsnError> {
    let chunks = mft.plan_chunks_with_options(config.chunk_plan_options());
    let iter_options = config.iter_options();
    let record_table_len = record_count_hint(mft, config);
    let mut records = Vec::with_capacity(record_table_len);
    records.resize_with(record_table_len, || None);
    let mut children_by_parent = FxHashMap::default();
    children_by_parent.reserve((record_count_hint(mft, config) / 8).max(1024));

    {
        let mut targets = BenchTargets {
            records: &mut records,
            children_by_parent: &mut children_by_parent,
        };
        mft.parallel()
            .chunks(chunks)
            .scan_options(iter_options)
            .workers(config.worker_count)
            .scheduling(config.scheduling)
            .fold_chunks(
                new_partial_ingest,
                |partial, entry| {
                    ingest_raw_entry_partial(entry, partial);
                    Ok(())
                },
                |partial| {
                    merge_partial_ingest(partial, &mut targets);
                    Ok(())
                },
            )?;
    }

    Ok(summarize_targets(&records, &children_by_parent))
}

/// Run the same parallel ingest workload while collecting attr-list enrichment counters.
pub fn run_parallel_ingest_with_attr_list_profile(
    mft: &RawMft<'_>,
    config: &BenchConfig,
) -> Result<(BenchSummary, BenchAttrListProfile), UsnError> {
    let (summary, profiles) = run_parallel_ingest_with_profiles(mft, config, true, false)?;
    Ok((
        summary,
        profiles
            .attr_list
            .expect("attr-list profile should be present when explicitly requested"),
    ))
}

/// Run the same parallel ingest workload while collecting any enabled exact-match profiles.
pub fn run_parallel_ingest_with_profiles(
    mft: &RawMft<'_>,
    config: &BenchConfig,
    collect_attr_list_profile: bool,
    collect_scheduling_profile: bool,
) -> Result<(BenchSummary, BenchRunProfiles), UsnError> {
    let _attr_guard = collect_attr_list_profile.then(attr_list_profile::start);
    let _schedule_guard = collect_scheduling_profile.then(schedule_profile::start);
    let summary = run_parallel_ingest(mft, config)?;
    Ok((
        summary,
        BenchRunProfiles {
            attr_list: collect_attr_list_profile.then(snapshot_attr_list_profile),
            scheduling: collect_scheduling_profile.then(snapshot_scheduling_profile),
        },
    ))
}

/// Run the same ingest workload serially for comparison.
pub fn run_serial_ingest(mft: &RawMft<'_>, config: &BenchConfig) -> Result<BenchSummary, UsnError> {
    let iter = mft.iter_with_options(config.iter_options())?;
    let record_table_len = record_count_hint(mft, config);
    let mut records = Vec::with_capacity(record_table_len);
    records.resize_with(record_table_len, || None);
    let mut children_by_parent = FxHashMap::default();
    children_by_parent.reserve((record_count_hint(mft, config) / 8).max(1024));

    {
        let mut targets = BenchTargets {
            records: &mut records,
            children_by_parent: &mut children_by_parent,
        };
        for item in iter {
            ingest_iter_entry(item?, &mut targets);
        }
    }

    Ok(summarize_targets(&records, &children_by_parent))
}

/// Open the requested drive, or report why the bench should be skipped.
pub fn open_volume(drive: char) -> Option<Volume> {
    match Volume::from_drive_letter(drive) {
        Ok(volume) => Some(volume),
        Err(UsnError::NotElevated) => {
            eprintln!("skipping bench: requires admin privileges");
            None
        }
        Err(error) => {
            eprintln!("skipping bench: {error}");
            None
        }
    }
}

/// Print attr-list profiling counters in a stable human-readable form.
pub fn print_attr_list_profile(profile: &BenchAttrListProfile, elapsed: Duration) {
    println!("attr_list profile");
    println!(
        "  records_scanned:                 {}",
        profile.records_scanned
    );
    println!(
        "  records_with_attr_list:          {} ({:.2}%)",
        profile.records_with_attr_list,
        profile.attr_list_hit_rate() * 100.0
    );
    println!(
        "  resident_attr_lists:             {} (payload {} bytes)",
        profile.resident_attr_lists, profile.resident_attr_list_bytes
    );
    println!(
        "  nonresident_attr_lists:          {} (payload {} bytes)",
        profile.nonresident_attr_lists, profile.nonresident_attr_list_bytes
    );
    println!(
        "  records_needing_enrich:          {} ({:.2}% of attr-list records)",
        profile.records_needing_enrich,
        profile.enrich_rate_given_attr_list() * 100.0
    );
    println!(
        "  records_skipped_after_need_check:{}",
        profile.records_skipped_after_need_check
    );
    println!(
        "  enrichment_calls:                {}",
        profile.enrichment_calls
    );
    println!(
        "  extension_records_referenced:    {} ({:.2} per call)",
        profile.extension_records_referenced,
        profile.referenced_extensions_per_enrich()
    );
    println!(
        "  extension_records_loaded:        {} ({:.2} per call)",
        profile.extension_records_loaded,
        profile.loaded_extensions_per_enrich()
    );
    println!(
        "  enrich_wall_time_ms:             {:.3} ({:.2}% of elapsed)",
        profile.enrich_wall_time.as_secs_f64() * 1_000.0,
        ratio_duration(profile.enrich_wall_time, elapsed) * 100.0
    );
    println!(
        "  attr_list_materialize_ms:        {:.3}",
        profile.attr_list_materialize_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  extension_ref_discovery_ms:      {:.3}",
        profile.extension_record_discovery_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  extension_load_attempts:         {} ({:.2} per call)",
        profile.extension_record_load_attempts,
        profile.load_attempts_per_enrich()
    );
    println!(
        "  extension_load_ms:               {:.3}",
        profile.extension_record_load_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  extension_offset_lookup_ms:      {:.3}",
        profile.extension_offset_lookup_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  extension_record_read_ms:        {:.3}",
        profile.extension_record_read_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  extension_record_parse_ms:       {:.3}",
        profile.extension_record_parse_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  extension_entry_build_ms:        {:.3}",
        profile.extension_entry_build_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  unique_extension_records:        {}",
        profile.unique_extension_records_loaded
    );
    println!(
        "  records_reloaded:                {}",
        profile.records_reloaded
    );
    println!(
        "  repeated_extension_loads:        {} ({:.2}% of load attempts)",
        profile.repeated_extension_loads,
        profile.repeated_extension_load_rate() * 100.0
    );
    println!(
        "  max_loads_single_extension:      {}",
        profile.max_loads_for_single_extension_record
    );
    println!(
        "  extension_offset_samples:        {}",
        profile.extension_offset_sequence_samples
    );
    println!(
        "  extension_exact_adjacent:        {} ({:.2}%)",
        profile.extension_offset_exact_adjacent,
        profile.exact_adjacent_extension_load_rate() * 100.0
    );
    println!(
        "  extension_jump_le_1_mib:         {} ({:.2}%)",
        profile.extension_offset_jump_le_1_mib,
        profile.extension_jump_le_1_mib_rate() * 100.0
    );
    println!(
        "  extension_jump_le_8_mib:         {} ({:.2}%)",
        profile.extension_offset_jump_le_8_mib,
        profile.extension_jump_le_8_mib_rate() * 100.0
    );
    println!(
        "  extension_jump_gt_64_mib:        {} ({:.2}%)",
        profile.extension_offset_jump_gt_64_mib,
        profile.extension_jump_gt_64_mib_rate() * 100.0
    );
    println!(
        "  extension_forward_moves:         {}",
        profile.extension_offset_forward
    );
    println!(
        "  extension_backward_moves:        {}",
        profile.extension_offset_backward
    );
    println!(
        "  extension_avg_abs_jump_mib:      {:.3}",
        profile.avg_extension_abs_jump_bytes() / (1024.0 * 1024.0)
    );
    println!(
        "  extension_max_abs_jump_mib:      {:.3}",
        profile.extension_offset_max_abs_jump_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "  base_to_extension_samples:       {}",
        profile.base_to_extension_samples
    );
    println!(
        "  base_to_extension_le_1_mib:      {} ({:.2}%)",
        profile.base_to_extension_jump_le_1_mib,
        profile.base_to_extension_le_1_mib_rate() * 100.0
    );
    println!(
        "  base_to_extension_le_8_mib:      {} ({:.2}%)",
        profile.base_to_extension_jump_le_8_mib,
        profile.base_to_extension_le_8_mib_rate() * 100.0
    );
    println!(
        "  base_to_extension_gt_64_mib:     {} ({:.2}%)",
        profile.base_to_extension_jump_gt_64_mib,
        profile.base_to_extension_gt_64_mib_rate() * 100.0
    );
    println!(
        "  base_to_extension_avg_jump_mib:  {:.3}",
        profile.avg_base_to_extension_abs_jump_bytes() / (1024.0 * 1024.0)
    );
    println!(
        "  base_to_extension_max_jump_mib:  {:.3}",
        profile.base_to_extension_max_abs_jump_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "  extension_within_chunk:          {} ({:.2}% of loads)",
        profile.extension_within_current_chunk,
        profile.within_current_chunk_rate() * 100.0
    );
    println!(
        "  extension_outside_chunk:         {}",
        profile.extension_outside_current_chunk
    );
    println!(
        "  extension_visit_ms:              {:.3}",
        profile.extension_record_visit_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  ext_name_materialize_calls:      {} ({:.2} UTF-16 units/call)",
        profile.extension_file_name_materialize_calls,
        profile.code_units_per_materialized_name()
    );
    println!(
        "  ext_name_materialize_ms:         {:.3}",
        profile.extension_file_name_materialize_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  link_merge_calls:                {} ({:.2} output links/call)",
        profile.link_merge_calls,
        profile.output_links_per_merge()
    );
    println!(
        "  link_merge_output_links:         {}",
        profile.link_merge_output_links
    );
    println!(
        "  link_inline_name_copies:         {}",
        profile.link_inline_name_copies
    );
    println!(
        "  link_slice_copy_inputs:          {}",
        profile.link_slice_copy_inputs
    );
    println!(
        "  link_merge_ms:                   {:.3}",
        profile.link_merge_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  data_merge_ms:                   {:.3}",
        profile.data_merge_time.as_secs_f64() * 1_000.0
    );
    println!(
        "  selected_name_replacements:      {}",
        profile.selected_name_replacements
    );
    println!(
        "  selected_name_replace_ms:        {:.3}",
        profile.selected_name_replace_time.as_secs_f64() * 1_000.0
    );
}

/// Print optional scheduling telemetry in a stable human-readable form.
pub fn print_scheduling_profile(profile: &BenchSchedulingProfile, elapsed: Duration) {
    println!("scheduling profile");
    println!("  workers:                  {}", profile.workers.len());
    println!(
        "  worker_elapsed_spread_ms: {:.3}",
        (profile.max_worker_elapsed().as_secs_f64() - profile.min_worker_elapsed().as_secs_f64())
            * 1_000.0
    );
    for worker in &profile.workers {
        println!(
            "  worker[{:<2}] chunks={} records={} avg_chunk_ms={:.3} total_ms={:.3} max_chunk_ms={:.3} est_cost_total={} est_cost_max={}",
            worker.worker_index,
            worker.chunks,
            worker.records,
            worker.avg_chunk_elapsed().as_secs_f64() * 1_000.0,
            worker.total_elapsed.as_secs_f64() * 1_000.0,
            worker.max_chunk_elapsed.as_secs_f64() * 1_000.0,
            worker.total_estimated_cost,
            worker.max_estimated_cost,
        );
    }
    if !profile.band_decisions.is_empty() {
        println!(
            "  prediction_top_overlap:   {}/{}",
            profile.predicted_actual_top_overlap,
            profile.compared_top_k
        );
        println!(
            "  prediction_top_hit_rate:  {:.2}%",
            profile.actual_top_hit_rate() * 100.0
        );
        println!(
            "  top_half_hits:            {}/{} ({:.2}%)",
            profile.actual_top_in_predicted_top_half,
            profile.compared_top_k,
            profile.actual_top_half_hit_rate() * 100.0
        );
        println!(
            "  top_quarter_hits:         {}/{} ({:.2}%)",
            profile.actual_top_in_predicted_top_quarter,
            profile.compared_top_k,
            profile.actual_top_quarter_hit_rate() * 100.0
        );
        println!(
            "  top_k_missed_actual:      {}",
            profile.actual_top_missed_by_predicted_top_k
        );
        println!(
            "  top_k_false_positives:    {}",
            profile.predicted_top_false_positives
        );
        println!(
            "  worst_pred_rank_in_top_k: {}",
            profile.actual_top_worst_predicted_rank
        );
        println!(
            "  adaptive_band_decisions:  total={} static={} observed={}",
            profile.band_decisions.len(),
            profile.static_estimate_band_count(),
            profile.observed_model_band_count(),
        );
        println!("  adaptive_bands:");
        for band in &profile.band_decisions {
            println!(
                "    band={} chunks={} samples_before={} source={} front=idx:{} pred={} back=idx:{} pred={}",
                band.band_index,
                band.chunk_count,
                band.sample_count_before,
                band.prediction_source.label(),
                band.front_chunk_index,
                format_prediction_key(band.prediction_source, band.front_prediction_key),
                band.back_chunk_index,
                format_prediction_key(band.prediction_source, band.back_prediction_key),
            );
        }
    }
    println!("  slowest_chunks:");
    for (actual_rank, chunk) in profile.slowest_chunks.iter().enumerate() {
        println!(
            "    actual_rank={} pred_rank={} idx={} worker={} claim={} band={} band_pos={} source={} pred={} records={} elapsed_ms={:.3} elapsed_pct={:.2}% cost={} used={} transitions={} attr_lists={} nonresident={} enrich={} ext_refs={} sparse={} discontinuities={} segments={} span_mib={:.3} phys_start={}",
            actual_rank + 1,
            chunk.predicted_rank,
            chunk.chunk_index,
            chunk.worker_index,
            chunk.claim_order,
            chunk
                .band_index
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            chunk
                .band_position
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            chunk.prediction_source.label(),
            format_prediction_key(chunk.prediction_source, chunk.predicted_order_key),
            chunk.record_len(),
            chunk.elapsed.as_secs_f64() * 1_000.0,
            ratio_duration(chunk.elapsed, elapsed) * 100.0,
            chunk.estimated_cost_score(),
            chunk.estimated_used_records,
            chunk.estimated_usage_transitions,
            chunk.estimated_attr_list_records,
            chunk.estimated_nonresident_attr_lists,
            chunk.estimated_enrich_candidates,
            chunk.estimated_referenced_extension_records,
            chunk.estimated_sparse_segments,
            chunk.estimated_discontinuities,
            chunk.estimated_overlapped_segments,
            chunk.estimated_physical_span_bytes as f64 / (1024.0 * 1024.0),
            chunk
                .physical_start_offset
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_owned()),
        );
    }
}

fn snapshot_attr_list_profile() -> BenchAttrListProfile {
    let snapshot = attr_list_profile::snapshot();
    BenchAttrListProfile {
        records_scanned: snapshot.records_scanned,
        records_with_attr_list: snapshot.records_with_attr_list,
        resident_attr_lists: snapshot.resident_attr_lists,
        nonresident_attr_lists: snapshot.nonresident_attr_lists,
        records_needing_enrich: snapshot.records_needing_enrich,
        records_skipped_after_need_check: snapshot.records_skipped_after_need_check,
        enrichment_calls: snapshot.enrichment_calls,
        extension_records_referenced: snapshot.extension_records_referenced,
        extension_records_loaded: snapshot.extension_records_loaded,
        resident_attr_list_bytes: snapshot.resident_attr_list_bytes,
        nonresident_attr_list_bytes: snapshot.nonresident_attr_list_bytes,
        enrich_wall_time: snapshot.enrich_wall_time,
        attr_list_materialize_time: snapshot.attr_list_materialize_time,
        extension_record_discovery_time: snapshot.extension_record_discovery_time,
        extension_record_load_attempts: snapshot.extension_record_load_attempts,
        extension_record_load_time: snapshot.extension_record_load_time,
        extension_offset_lookup_time: snapshot.extension_offset_lookup_time,
        extension_record_read_time: snapshot.extension_record_read_time,
        extension_record_parse_time: snapshot.extension_record_parse_time,
        extension_entry_build_time: snapshot.extension_entry_build_time,
        extension_record_visit_time: snapshot.extension_record_visit_time,
        extension_file_name_materialize_calls: snapshot.extension_file_name_materialize_calls,
        extension_file_name_code_units: snapshot.extension_file_name_code_units,
        extension_file_name_materialize_time: snapshot.extension_file_name_materialize_time,
        link_merge_calls: snapshot.link_merge_calls,
        link_merge_output_links: snapshot.link_merge_output_links,
        link_inline_name_copies: snapshot.link_inline_name_copies,
        link_slice_copy_inputs: snapshot.link_slice_copy_inputs,
        link_merge_time: snapshot.link_merge_time,
        data_merge_time: snapshot.data_merge_time,
        selected_name_replacements: snapshot.selected_name_replacements,
        selected_name_replace_time: snapshot.selected_name_replace_time,
        unique_extension_records_loaded: snapshot.unique_extension_records_loaded,
        records_reloaded: snapshot.records_reloaded,
        repeated_extension_loads: snapshot.repeated_extension_loads,
        max_loads_for_single_extension_record: snapshot.max_loads_for_single_extension_record,
        extension_offset_sequence_samples: snapshot.extension_offset_sequence_samples,
        extension_offset_exact_adjacent: snapshot.extension_offset_exact_adjacent,
        extension_offset_forward: snapshot.extension_offset_forward,
        extension_offset_backward: snapshot.extension_offset_backward,
        extension_offset_jump_le_1_mib: snapshot.extension_offset_jump_le_1_mib,
        extension_offset_jump_le_8_mib: snapshot.extension_offset_jump_le_8_mib,
        extension_offset_jump_gt_64_mib: snapshot.extension_offset_jump_gt_64_mib,
        extension_offset_abs_jump_bytes: snapshot.extension_offset_abs_jump_bytes,
        extension_offset_max_abs_jump_bytes: snapshot.extension_offset_max_abs_jump_bytes,
        base_to_extension_samples: snapshot.base_to_extension_samples,
        base_to_extension_jump_le_1_mib: snapshot.base_to_extension_jump_le_1_mib,
        base_to_extension_jump_le_8_mib: snapshot.base_to_extension_jump_le_8_mib,
        base_to_extension_jump_gt_64_mib: snapshot.base_to_extension_jump_gt_64_mib,
        base_to_extension_abs_jump_bytes: snapshot.base_to_extension_abs_jump_bytes,
        base_to_extension_max_abs_jump_bytes: snapshot.base_to_extension_max_abs_jump_bytes,
        extension_within_current_chunk: snapshot.extension_within_current_chunk,
        extension_outside_current_chunk: snapshot.extension_outside_current_chunk,
    }
}

fn snapshot_scheduling_profile() -> BenchSchedulingProfile {
    let snapshot = schedule_profile::snapshot();
    BenchSchedulingProfile {
        workers: snapshot
            .workers
            .into_iter()
            .filter(|worker| worker.chunks != 0)
            .map(|worker| BenchWorkerSchedulingProfile {
                worker_index: worker.worker_index,
                chunks: worker.chunks,
                records: worker.records,
                covered_bytes: worker.covered_bytes,
                total_elapsed: worker.total_elapsed,
                max_chunk_elapsed: worker.max_chunk_elapsed,
                total_estimated_cost: worker.total_estimated_cost,
                max_estimated_cost: worker.max_estimated_cost,
            })
            .collect(),
        band_decisions: snapshot
            .band_decisions
            .into_iter()
            .map(|band| BenchBandDecisionProfile {
                band_index: band.band_index,
                chunk_count: band.chunk_count,
                sample_count_before: band.sample_count_before,
                prediction_source: map_prediction_source(band.prediction_source),
                front_chunk_index: band.front_chunk_index,
                front_prediction_key: band.front_prediction_key,
                back_chunk_index: band.back_chunk_index,
                back_prediction_key: band.back_prediction_key,
            })
            .collect(),
        compared_top_k: snapshot.compared_top_k,
        predicted_actual_top_overlap: snapshot.predicted_actual_top_overlap,
        actual_top_in_predicted_top_half: snapshot.actual_top_in_predicted_top_half,
        actual_top_in_predicted_top_quarter: snapshot.actual_top_in_predicted_top_quarter,
        actual_top_missed_by_predicted_top_k: snapshot.actual_top_missed_by_predicted_top_k,
        predicted_top_false_positives: snapshot.predicted_top_false_positives,
        actual_top_worst_predicted_rank: snapshot.actual_top_worst_predicted_rank,
        slowest_chunks: snapshot
            .slowest_chunks
            .into_iter()
            .map(|chunk| BenchSlowChunkProfile {
                worker_index: chunk.worker_index,
                chunk_index: chunk.chunk_index,
                start_record: chunk.chunk.start_record,
                end_record: chunk.chunk.end_record,
                elapsed: chunk.elapsed,
                claim_order: chunk.claim_order,
                band_index: chunk.band_index,
                band_position: chunk.band_position,
                prediction_source: map_prediction_source(chunk.prediction_source),
                predicted_order_key: chunk.predicted_order_key,
                predicted_rank: chunk.predicted_rank,
                estimated_used_records: chunk.estimated_cost.used_records,
                estimated_usage_transitions: chunk.estimated_cost.usage_transitions,
                estimated_attr_list_records: chunk.estimated_cost.attr_list_records,
                estimated_nonresident_attr_lists: chunk.estimated_cost.nonresident_attr_lists,
                estimated_enrich_candidates: chunk.estimated_cost.enrich_candidates,
                estimated_referenced_extension_records: chunk.estimated_cost.referenced_extension_records,
                estimated_sparse_segments: chunk.estimated_cost.sparse_segments,
                estimated_discontinuities: chunk.estimated_cost.discontinuities,
                estimated_overlapped_segments: chunk.estimated_cost.overlapped_segments,
                estimated_physical_span_bytes: chunk.estimated_cost.physical_span_bytes,
                covered_bytes: chunk.estimated_cost.covered_bytes,
                physical_start_offset: chunk.physical_start_offset,
            })
            .collect(),
    }
}

fn map_prediction_source(source: schedule_profile::PredictionSource) -> BenchPredictionSource {
    match source {
        schedule_profile::PredictionSource::StaticEstimate => BenchPredictionSource::StaticEstimate,
        schedule_profile::PredictionSource::ObservedModel => BenchPredictionSource::ObservedModel,
    }
}

fn format_prediction_key(source: BenchPredictionSource, key: u64) -> String {
    match source {
        BenchPredictionSource::StaticEstimate => format!("score:{}", key),
        BenchPredictionSource::ObservedModel => format!("ms:{:.3}", key as f64 / 1_000.0),
    }
}

/// Estimate the record-table size for the configured scan range.
fn record_count_hint(mft: &RawMft<'_>, config: &BenchConfig) -> usize {
    let total = mft.record_count();
    let end = config.end_record.unwrap_or(total).min(total);
    end.min(usize::MAX as u64) as usize
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn ratio_duration(numerator: Duration, denominator: Duration) -> f64 {
    if denominator.is_zero() {
        0.0
    } else {
        numerator.as_secs_f64() / denominator.as_secs_f64()
    }
}

/// Summarize the captured record and child-link tables.
fn summarize_targets(
    records: &[Option<BenchNodeMeta>],
    children_by_parent: &FxHashMap<u64, Vec<BenchChildLink>>,
) -> BenchSummary {
    let mut summary = BenchSummary {
        parent_buckets: children_by_parent.len(),
        ..BenchSummary::default()
    };
    for metadata in records.iter().flatten() {
        summary.records += 1;
        summary.logical_bytes = summary.logical_bytes.saturating_add(metadata.size);
        summary.allocated_bytes = summary
            .allocated_bytes
            .saturating_add(metadata.allocated_size);
    }
    for child_links in children_by_parent.values() {
        summary.child_links += child_links.len();
        for child_link in child_links {
            let _ = child_link.child_record;
        }
    }
    summary
}

/// Create an empty worker-local accumulator sized for one chunk.
fn new_partial_ingest(chunk: RawMftWorkChunk) -> PartialIngest {
    let capacity = chunk.record_len().min(usize::MAX as u64) as usize;
    PartialIngest {
        records: Vec::with_capacity(capacity),
        child_links: Vec::with_capacity(capacity),
    }
}

/// Merge one worker-local chunk result into the shared benchmark targets.
fn merge_partial_ingest(partial: PartialIngest, targets: &mut BenchTargets<'_>) {
    for (record_number, metadata) in partial.records {
        if let Some(record) = targets.records.get_mut(record_number as usize) {
            *record = Some(metadata);
        }
    }
    let mut child_links = partial.child_links.into_iter().peekable();
    while let Some((parent_record, child_link)) = child_links.next() {
        let children = targets
            .children_by_parent
            .entry(parent_record)
            .or_insert_with(|| Vec::with_capacity(1));
        children.push(child_link);
        while let Some((next_parent, _)) = child_links.peek() {
            if *next_parent != parent_record {
                break;
            }
            if let Some((_, child_link)) = child_links.next() {
                children.push(child_link);
            }
        }
    }
}

/// Convert a batch entry into the compact benchmark representation.
fn ingest_raw_entry_partial(entry: RawMftBatchEntry, partial: &mut PartialIngest) {
    if entry.base_record_reference != 0 {
        return;
    }

    let metadata = BenchNodeMeta {
        size: if entry.is_directory {
            0
        } else {
            entry.real_size
        },
        allocated_size: if entry.is_directory {
            0
        } else {
            entry.allocated_size
        },
    };
    let record_number = entry.record_number;
    partial.records.push((record_number, metadata));

    for_each_visible_link(
        entry.parent_reference,
        entry.namespace,
        entry.file_name.as_os_str(),
        &entry.links,
        |parent_reference, _file_name| {
            let Some(parent_record) = parent_reference.record_number() else {
                return;
            };
            partial.child_links.push((
                parent_record,
                BenchChildLink {
                    child_record: record_number,
                },
            ));
        },
    );
}

/// Convert a full iterator entry into the compact benchmark representation.
fn ingest_iter_entry(entry: RawMftEntry, targets: &mut BenchTargets<'_>) {
    if entry.base_record_reference != 0 {
        return;
    }

    let metadata = BenchNodeMeta {
        size: if entry.is_directory {
            0
        } else {
            entry.real_size
        },
        allocated_size: if entry.is_directory {
            0
        } else {
            entry.allocated_size
        },
    };
    let record_number = entry.record_number;
    if let Some(record) = targets.records.get_mut(record_number as usize) {
        *record = Some(metadata);
    }

    for_each_visible_link(
        entry.parent_reference,
        entry.namespace,
        entry.file_name.as_os_str(),
        &entry.links,
        |parent_reference, _file_name| {
            let Some(parent_record) = parent_reference.record_number() else {
                return;
            };
            targets
                .children_by_parent
                .entry(parent_record)
                .or_default()
                .push(BenchChildLink {
                    child_record: record_number,
                });
        },
    );
}

/// Visit the visible file-name links for a record and suppress shadowed names.
fn for_each_visible_link<F>(
    parent_reference: Fid,
    _namespace: FileNameNamespace,
    file_name: &OsStr,
    all_links: &[RawMftLink],
    mut visit: F,
) where
    F: FnMut(Fid, &OsStr),
{
    if all_links.is_empty() {
        if !file_name.is_empty() {
            visit(parent_reference, file_name);
        }
        return;
    }

    let mut parent_visibility = Vec::with_capacity(all_links.len().min(4));
    for link in all_links {
        if link.file_name.is_empty() {
            continue;
        }
        let state = parent_visibility_state_mut(&mut parent_visibility, link.parent_reference);
        if link.namespace != FileNameNamespace::Dos {
            state.has_non_dos = true;
        }
        if matches!(
            link.namespace,
            FileNameNamespace::Win32 | FileNameNamespace::Win32AndDos
        ) {
            state.has_win32ish = true;
        }
    }

    let mut emitted_any = false;
    let mut emitted_links = Vec::with_capacity(all_links.len().min(4));
    for link in all_links {
        let Some(state) = parent_visibility_for(&parent_visibility, link.parent_reference) else {
            continue;
        };
        if !link_is_visible_for_parent(link, state) {
            continue;
        }
        if emitted_links.iter().any(|(emitted_parent, emitted_name)| {
            *emitted_parent == link.parent_reference && *emitted_name == link.file_name.as_os_str()
        }) {
            continue;
        }
        emitted_any = true;
        emitted_links.push((link.parent_reference, link.file_name.as_os_str()));
        visit(link.parent_reference, link.file_name.as_os_str());
    }

    if !emitted_any && !file_name.is_empty() {
        visit(parent_reference, file_name);
    }
}

#[derive(Debug, Clone, Copy)]
struct ParentVisibilityState {
    parent_reference: Fid,
    has_non_dos: bool,
    has_win32ish: bool,
}

fn parent_visibility_state_mut(
    states: &mut Vec<ParentVisibilityState>,
    parent_reference: Fid,
) -> &mut ParentVisibilityState {
    if let Some(index) = states
        .iter()
        .position(|state| state.parent_reference == parent_reference)
    {
        return &mut states[index];
    }

    states.push(ParentVisibilityState {
        parent_reference,
        has_non_dos: false,
        has_win32ish: false,
    });
    states
        .last_mut()
        .expect("just pushed parent visibility state")
}

fn parent_visibility_for(
    states: &[ParentVisibilityState],
    parent_reference: Fid,
) -> Option<ParentVisibilityState> {
    states
        .iter()
        .find(|state| state.parent_reference == parent_reference)
        .copied()
}

fn link_is_visible_for_parent(link: &RawMftLink, parent_state: ParentVisibilityState) -> bool {
    if link.file_name.is_empty() {
        return false;
    }
    if link.namespace == FileNameNamespace::Posix
        && parent_state.has_win32ish
    {
        return false;
    }
    if link.namespace == FileNameNamespace::Dos && parent_state.has_non_dos {
        return false;
    }
    true
}

/// Pick the default drive from the environment, falling back to `C:`.
fn pick_drive() -> char {
    env::var("USN_RAW_MFT_BENCH_DRIVE")
        .ok()
        .or_else(|| env::var("USN_TEST_DRIVE").ok())
        .and_then(|value| value.chars().next())
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or('C')
}

/// Choose the default worker count for the benchmark.
fn default_worker_count() -> NonZeroUsize {
    let available = thread::available_parallelism()
        .ok()
        .map(NonZeroUsize::get)
        .unwrap_or(1);
    // The current Criterion worker sweeps on a large C: volume keep
    // `skip_unused(true)` in both chunk planning and scanning, but chunk
    // planning now stays dense and only drops fully unused logical bands.
    // With the current 2,048-record default chunk size that produces about
    // 1,329 planned chunks on the measured live volume and still settles into
    // its fastest region around 10-11 workers with dynamic scheduling. Cap
    // the benchmark default at 10 so unattended runs start near the measured
    // sweet spot instead of blindly using every logical CPU.
    nonzero_usize(available.clamp(1, 10))
}

/// Parse a non-zero `usize` from an environment variable.
fn parse_env_nonzero_usize(name: &str, default: NonZeroUsize) -> NonZeroUsize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(NonZeroUsize::new)
        .unwrap_or(default)
}

/// Parse a non-zero `u64` from an environment variable.
fn parse_env_nonzero_u64(name: &str, default: NonZeroU64) -> NonZeroU64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(NonZeroU64::new)
        .unwrap_or(default)
}

fn parse_nonzero_usize_list(name: &str) -> Vec<NonZeroUsize> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter_map(|item| item.trim().parse::<usize>().ok())
                .filter_map(NonZeroUsize::new)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_default()
}

fn parse_bench_scheduling(value: Option<&str>, default: BenchScheduling) -> BenchScheduling {
    value
        .and_then(parse_bench_scheduling_token)
        .unwrap_or(default)
}

fn parse_bench_scheduling_token(value: &str) -> Option<BenchScheduling> {
    if value.eq_ignore_ascii_case("dynamic") {
        Some(BenchScheduling::Dynamic)
    } else if value.eq_ignore_ascii_case("dynamic-physical-order")
        || value.eq_ignore_ascii_case("dynamic_physical_order")
        || value.eq_ignore_ascii_case("physical")
        || value.eq_ignore_ascii_case("extent")
    {
        Some(BenchScheduling::DynamicPhysicalOrder)
    } else if value.eq_ignore_ascii_case("dynamic-cost-banded")
        || value.eq_ignore_ascii_case("dynamic_cost_banded")
        || value.eq_ignore_ascii_case("cost-banded")
        || value.eq_ignore_ascii_case("cost")
        || value.eq_ignore_ascii_case("banded")
    {
        Some(BenchScheduling::DynamicCostBanded)
    } else if value.eq_ignore_ascii_case("dynamic-observed-adaptive")
        || value.eq_ignore_ascii_case("dynamic_observed_adaptive")
        || value.eq_ignore_ascii_case("observed-adaptive")
        || value.eq_ignore_ascii_case("observed_adaptive")
        || value.eq_ignore_ascii_case("adaptive")
        || value.eq_ignore_ascii_case("observed")
    {
        Some(BenchScheduling::DynamicObservedAdaptive)
    } else if value.eq_ignore_ascii_case("contiguous") {
        Some(BenchScheduling::Contiguous)
    } else {
        None
    }
}

/// Convert a `usize` into a non-zero `usize`, clamping zero to one.
fn nonzero_usize(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or(NonZeroUsize::MIN)
}

/// Convert a `u64` into a non-zero `u64`, clamping zero to one.
fn nonzero_u64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn sample_config() -> BenchConfig {
        BenchConfig {
            drive: 'C',
            worker_count: NonZeroUsize::new(11).expect("worker count must be non-zero"),
            main_buffer_bytes: NonZeroUsize::new(DEFAULT_MAIN_BUFFER_BYTES)
                .expect("main buffer must be non-zero"),
            attr_buffer_bytes: NonZeroUsize::new(DEFAULT_ATTR_BUFFER_BYTES)
                .expect("attr buffer must be non-zero"),
            chunk_records: NonZeroU64::new(DEFAULT_CHUNK_RECORDS)
                .expect("chunk size must be non-zero"),
            start_record: FIRST_NORMAL_RECORD,
            end_record: None,
            scheduling: ChunkScheduling::Dynamic,
        }
    }

    #[test]
    fn benchmark_scan_and_chunk_defaults_both_skip_unused() {
        let config = sample_config();
        let iter_options = config.iter_options();
        let chunk_options = config.chunk_plan_options();

        assert!(iter_options.skip_unused());
        assert!(iter_options.skip_extension_records());
        assert!(chunk_options.skip_unused());
    }

    #[test]
    fn parses_observed_adaptive_scheduling_tokens() {
        assert_eq!(
            parse_bench_scheduling_token("dynamic-observed-adaptive"),
            Some(BenchScheduling::DynamicObservedAdaptive)
        );
        assert_eq!(
            parse_bench_scheduling_token("adaptive"),
            Some(BenchScheduling::DynamicObservedAdaptive)
        );
    }

    fn visible_links(
        parent_reference: Fid,
        file_name: &str,
        all_links: &[RawMftLink],
    ) -> Vec<(Fid, OsString)> {
        let mut visible = Vec::new();
        for_each_visible_link(
            parent_reference,
            FileNameNamespace::Win32,
            OsStr::new(file_name),
            all_links,
            |parent, name| visible.push((parent, name.to_os_string())),
        );
        visible
    }

    #[test]
    fn visible_links_suppress_shadowed_dos_and_posix_names() {
        let parent = Fid::new(5);
        let links = vec![
            RawMftLink {
                parent_reference: parent,
                namespace: FileNameNamespace::Dos,
                file_name: OsString::from("FILE~1.TXT"),
            },
            RawMftLink {
                parent_reference: parent,
                namespace: FileNameNamespace::Posix,
                file_name: OsString::from("file.txt"),
            },
            RawMftLink {
                parent_reference: parent,
                namespace: FileNameNamespace::Win32,
                file_name: OsString::from("file.txt"),
            },
        ];

        let visible = visible_links(parent, "fallback.txt", &links);

        assert_eq!(visible, vec![(parent, OsString::from("file.txt"))]);
    }

    #[test]
    fn visible_links_fall_back_to_inline_name_when_all_links_hidden() {
        let parent = Fid::new(7);
        let links = vec![RawMftLink {
            parent_reference: parent,
            namespace: FileNameNamespace::Win32,
            file_name: OsString::new(),
        }];

        let visible = visible_links(parent, "file.txt", &links);

        assert_eq!(visible, vec![(parent, OsString::from("file.txt"))]);
    }

    #[test]
    fn visible_links_keep_first_duplicate_visible_name_per_parent() {
        let parent = Fid::new(9);
        let links = vec![
            RawMftLink {
                parent_reference: parent,
                namespace: FileNameNamespace::Win32,
                file_name: OsString::from("same.txt"),
            },
            RawMftLink {
                parent_reference: parent,
                namespace: FileNameNamespace::Win32AndDos,
                file_name: OsString::from("same.txt"),
            },
        ];

        let visible = visible_links(parent, "fallback.txt", &links);

        assert_eq!(visible, vec![(parent, OsString::from("same.txt"))]);
    }
}
