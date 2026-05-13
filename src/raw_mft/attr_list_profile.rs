use std::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use rustc_hash::FxHashMap;

use super::{attr_list::AttrListEnrichStats, entry_build::AttributeListInfo};

static ENABLED: AtomicBool = AtomicBool::new(false);
static RECORDS_SCANNED: AtomicU64 = AtomicU64::new(0);
static RECORDS_WITH_ATTR_LIST: AtomicU64 = AtomicU64::new(0);
static RESIDENT_ATTR_LISTS: AtomicU64 = AtomicU64::new(0);
static NONRESIDENT_ATTR_LISTS: AtomicU64 = AtomicU64::new(0);
static RECORDS_NEEDING_ENRICH: AtomicU64 = AtomicU64::new(0);
static RECORDS_SKIPPED_AFTER_NEED_CHECK: AtomicU64 = AtomicU64::new(0);
static ENRICHMENT_CALLS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORDS_REFERENCED: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORDS_LOADED: AtomicU64 = AtomicU64::new(0);
static RESIDENT_ATTR_LIST_BYTES: AtomicU64 = AtomicU64::new(0);
static NONRESIDENT_ATTR_LIST_BYTES: AtomicU64 = AtomicU64::new(0);
static ENRICH_WALL_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static ATTR_LIST_MATERIALIZE_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORD_DISCOVERY_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORD_LOAD_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORD_LOAD_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_LOOKUP_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORD_READ_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORD_PARSE_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_ENTRY_BUILD_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_RECORD_VISIT_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_FILE_NAME_MATERIALIZE_CALLS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_FILE_NAME_CODE_UNITS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_FILE_NAME_MATERIALIZE_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static LINK_MERGE_CALLS: AtomicU64 = AtomicU64::new(0);
static LINK_MERGE_OUTPUT_LINKS: AtomicU64 = AtomicU64::new(0);
static LINK_INLINE_NAME_COPIES: AtomicU64 = AtomicU64::new(0);
static LINK_SLICE_COPY_INPUTS: AtomicU64 = AtomicU64::new(0);
static LINK_MERGE_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static DATA_MERGE_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static SELECTED_NAME_REPLACEMENTS: AtomicU64 = AtomicU64::new(0);
static SELECTED_NAME_REPLACE_TIME_NANOS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_SEQUENCE_SAMPLES: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_EXACT_ADJACENT: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_FORWARD: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_BACKWARD: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_JUMP_LE_1_MIB: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_JUMP_LE_8_MIB: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_JUMP_GT_64_MIB: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_ABS_JUMP_BYTES: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OFFSET_MAX_ABS_JUMP_BYTES: AtomicU64 = AtomicU64::new(0);
static BASE_TO_EXTENSION_SAMPLES: AtomicU64 = AtomicU64::new(0);
static BASE_TO_EXTENSION_JUMP_LE_1_MIB: AtomicU64 = AtomicU64::new(0);
static BASE_TO_EXTENSION_JUMP_LE_8_MIB: AtomicU64 = AtomicU64::new(0);
static BASE_TO_EXTENSION_JUMP_GT_64_MIB: AtomicU64 = AtomicU64::new(0);
static BASE_TO_EXTENSION_ABS_JUMP_BYTES: AtomicU64 = AtomicU64::new(0);
static BASE_TO_EXTENSION_MAX_ABS_JUMP_BYTES: AtomicU64 = AtomicU64::new(0);
static EXTENSION_WITHIN_CURRENT_CHUNK: AtomicU64 = AtomicU64::new(0);
static EXTENSION_OUTSIDE_CURRENT_CHUNK: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static EXTENSION_LOAD_DEPTH: Cell<u32> = const { Cell::new(0) };
    static LAST_EXTENSION_LOAD_OFFSET: Cell<Option<u64>> = const { Cell::new(None) };
    static CURRENT_BASE_RECORD_OFFSET: Cell<Option<u64>> = const { Cell::new(None) };
    static CURRENT_CHUNK_START_RECORD: Cell<Option<u64>> = const { Cell::new(None) };
    static CURRENT_CHUNK_END_RECORD: Cell<Option<u64>> = const { Cell::new(None) };
}

static EXTENSION_RECORD_LOAD_COUNTS: OnceLock<Mutex<FxHashMap<u64, u32>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AttrListProfileSnapshot {
    pub(crate) records_scanned: u64,
    pub(crate) records_with_attr_list: u64,
    pub(crate) resident_attr_lists: u64,
    pub(crate) nonresident_attr_lists: u64,
    pub(crate) records_needing_enrich: u64,
    pub(crate) records_skipped_after_need_check: u64,
    pub(crate) enrichment_calls: u64,
    pub(crate) extension_records_referenced: u64,
    pub(crate) extension_records_loaded: u64,
    pub(crate) resident_attr_list_bytes: u64,
    pub(crate) nonresident_attr_list_bytes: u64,
    pub(crate) enrich_wall_time: Duration,
    pub(crate) attr_list_materialize_time: Duration,
    pub(crate) extension_record_discovery_time: Duration,
    pub(crate) extension_record_load_attempts: u64,
    pub(crate) extension_record_load_time: Duration,
    pub(crate) extension_offset_lookup_time: Duration,
    pub(crate) extension_record_read_time: Duration,
    pub(crate) extension_record_parse_time: Duration,
    pub(crate) extension_entry_build_time: Duration,
    pub(crate) extension_record_visit_time: Duration,
    pub(crate) extension_file_name_materialize_calls: u64,
    pub(crate) extension_file_name_code_units: u64,
    pub(crate) extension_file_name_materialize_time: Duration,
    pub(crate) link_merge_calls: u64,
    pub(crate) link_merge_output_links: u64,
    pub(crate) link_inline_name_copies: u64,
    pub(crate) link_slice_copy_inputs: u64,
    pub(crate) link_merge_time: Duration,
    pub(crate) data_merge_time: Duration,
    pub(crate) selected_name_replacements: u64,
    pub(crate) selected_name_replace_time: Duration,
    pub(crate) unique_extension_records_loaded: u64,
    pub(crate) records_reloaded: u64,
    pub(crate) repeated_extension_loads: u64,
    pub(crate) max_loads_for_single_extension_record: u64,
    pub(crate) extension_offset_sequence_samples: u64,
    pub(crate) extension_offset_exact_adjacent: u64,
    pub(crate) extension_offset_forward: u64,
    pub(crate) extension_offset_backward: u64,
    pub(crate) extension_offset_jump_le_1_mib: u64,
    pub(crate) extension_offset_jump_le_8_mib: u64,
    pub(crate) extension_offset_jump_gt_64_mib: u64,
    pub(crate) extension_offset_abs_jump_bytes: u64,
    pub(crate) extension_offset_max_abs_jump_bytes: u64,
    pub(crate) base_to_extension_samples: u64,
    pub(crate) base_to_extension_jump_le_1_mib: u64,
    pub(crate) base_to_extension_jump_le_8_mib: u64,
    pub(crate) base_to_extension_jump_gt_64_mib: u64,
    pub(crate) base_to_extension_abs_jump_bytes: u64,
    pub(crate) base_to_extension_max_abs_jump_bytes: u64,
    pub(crate) extension_within_current_chunk: u64,
    pub(crate) extension_outside_current_chunk: u64,
}

#[must_use]
pub(crate) struct AttrListProfileGuard;

#[must_use]
pub(crate) struct ExtensionLoadGuard {
    active: bool,
}

impl Drop for AttrListProfileGuard {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Release);
    }
}

impl Drop for ExtensionLoadGuard {
    fn drop(&mut self) {
        if self.active {
            EXTENSION_LOAD_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
}

pub(crate) fn start() -> AttrListProfileGuard {
    reset();
    ENABLED.store(true, Ordering::Release);
    AttrListProfileGuard
}

pub(crate) fn is_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub(crate) fn snapshot() -> AttrListProfileSnapshot {
    let (unique_extension_records_loaded, records_reloaded, max_loads_for_single_extension_record) =
        extension_record_load_counts()
            .lock()
            .map(|counts| {
                let unique = counts.len() as u64;
                let reloaded = counts.values().filter(|&&count| count > 1).count() as u64;
                let max_loads = counts.values().copied().max().unwrap_or(0) as u64;
                (unique, reloaded, max_loads)
            })
            .unwrap_or_default();
    let extension_record_load_attempts = EXTENSION_RECORD_LOAD_ATTEMPTS.load(Ordering::Relaxed);

    AttrListProfileSnapshot {
        records_scanned: RECORDS_SCANNED.load(Ordering::Relaxed),
        records_with_attr_list: RECORDS_WITH_ATTR_LIST.load(Ordering::Relaxed),
        resident_attr_lists: RESIDENT_ATTR_LISTS.load(Ordering::Relaxed),
        nonresident_attr_lists: NONRESIDENT_ATTR_LISTS.load(Ordering::Relaxed),
        records_needing_enrich: RECORDS_NEEDING_ENRICH.load(Ordering::Relaxed),
        records_skipped_after_need_check: RECORDS_SKIPPED_AFTER_NEED_CHECK.load(Ordering::Relaxed),
        enrichment_calls: ENRICHMENT_CALLS.load(Ordering::Relaxed),
        extension_records_referenced: EXTENSION_RECORDS_REFERENCED.load(Ordering::Relaxed),
        extension_records_loaded: EXTENSION_RECORDS_LOADED.load(Ordering::Relaxed),
        resident_attr_list_bytes: RESIDENT_ATTR_LIST_BYTES.load(Ordering::Relaxed),
        nonresident_attr_list_bytes: NONRESIDENT_ATTR_LIST_BYTES.load(Ordering::Relaxed),
        enrich_wall_time: Duration::from_nanos(ENRICH_WALL_TIME_NANOS.load(Ordering::Relaxed)),
        attr_list_materialize_time: Duration::from_nanos(
            ATTR_LIST_MATERIALIZE_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_record_discovery_time: Duration::from_nanos(
            EXTENSION_RECORD_DISCOVERY_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_record_load_attempts,
        extension_record_load_time: Duration::from_nanos(
            EXTENSION_RECORD_LOAD_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_offset_lookup_time: Duration::from_nanos(
            EXTENSION_OFFSET_LOOKUP_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_record_read_time: Duration::from_nanos(
            EXTENSION_RECORD_READ_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_record_parse_time: Duration::from_nanos(
            EXTENSION_RECORD_PARSE_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_entry_build_time: Duration::from_nanos(
            EXTENSION_ENTRY_BUILD_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_record_visit_time: Duration::from_nanos(
            EXTENSION_RECORD_VISIT_TIME_NANOS.load(Ordering::Relaxed),
        ),
        extension_file_name_materialize_calls: EXTENSION_FILE_NAME_MATERIALIZE_CALLS
            .load(Ordering::Relaxed),
        extension_file_name_code_units: EXTENSION_FILE_NAME_CODE_UNITS.load(Ordering::Relaxed),
        extension_file_name_materialize_time: Duration::from_nanos(
            EXTENSION_FILE_NAME_MATERIALIZE_TIME_NANOS.load(Ordering::Relaxed),
        ),
        link_merge_calls: LINK_MERGE_CALLS.load(Ordering::Relaxed),
        link_merge_output_links: LINK_MERGE_OUTPUT_LINKS.load(Ordering::Relaxed),
        link_inline_name_copies: LINK_INLINE_NAME_COPIES.load(Ordering::Relaxed),
        link_slice_copy_inputs: LINK_SLICE_COPY_INPUTS.load(Ordering::Relaxed),
        link_merge_time: Duration::from_nanos(LINK_MERGE_TIME_NANOS.load(Ordering::Relaxed)),
        data_merge_time: Duration::from_nanos(DATA_MERGE_TIME_NANOS.load(Ordering::Relaxed)),
        selected_name_replacements: SELECTED_NAME_REPLACEMENTS.load(Ordering::Relaxed),
        selected_name_replace_time: Duration::from_nanos(
            SELECTED_NAME_REPLACE_TIME_NANOS.load(Ordering::Relaxed),
        ),
        unique_extension_records_loaded,
        records_reloaded,
        repeated_extension_loads: extension_record_load_attempts
            .saturating_sub(unique_extension_records_loaded),
        max_loads_for_single_extension_record,
        extension_offset_sequence_samples: EXTENSION_OFFSET_SEQUENCE_SAMPLES
            .load(Ordering::Relaxed),
        extension_offset_exact_adjacent: EXTENSION_OFFSET_EXACT_ADJACENT.load(Ordering::Relaxed),
        extension_offset_forward: EXTENSION_OFFSET_FORWARD.load(Ordering::Relaxed),
        extension_offset_backward: EXTENSION_OFFSET_BACKWARD.load(Ordering::Relaxed),
        extension_offset_jump_le_1_mib: EXTENSION_OFFSET_JUMP_LE_1_MIB.load(Ordering::Relaxed),
        extension_offset_jump_le_8_mib: EXTENSION_OFFSET_JUMP_LE_8_MIB.load(Ordering::Relaxed),
        extension_offset_jump_gt_64_mib: EXTENSION_OFFSET_JUMP_GT_64_MIB.load(Ordering::Relaxed),
        extension_offset_abs_jump_bytes: EXTENSION_OFFSET_ABS_JUMP_BYTES.load(Ordering::Relaxed),
        extension_offset_max_abs_jump_bytes: EXTENSION_OFFSET_MAX_ABS_JUMP_BYTES
            .load(Ordering::Relaxed),
        base_to_extension_samples: BASE_TO_EXTENSION_SAMPLES.load(Ordering::Relaxed),
        base_to_extension_jump_le_1_mib: BASE_TO_EXTENSION_JUMP_LE_1_MIB.load(Ordering::Relaxed),
        base_to_extension_jump_le_8_mib: BASE_TO_EXTENSION_JUMP_LE_8_MIB.load(Ordering::Relaxed),
        base_to_extension_jump_gt_64_mib: BASE_TO_EXTENSION_JUMP_GT_64_MIB.load(Ordering::Relaxed),
        base_to_extension_abs_jump_bytes: BASE_TO_EXTENSION_ABS_JUMP_BYTES.load(Ordering::Relaxed),
        base_to_extension_max_abs_jump_bytes: BASE_TO_EXTENSION_MAX_ABS_JUMP_BYTES
            .load(Ordering::Relaxed),
        extension_within_current_chunk: EXTENSION_WITHIN_CURRENT_CHUNK.load(Ordering::Relaxed),
        extension_outside_current_chunk: EXTENSION_OUTSIDE_CURRENT_CHUNK.load(Ordering::Relaxed),
    }
}

pub(crate) fn set_current_enrichment_base_context(
    base_offset: Option<u64>,
    chunk_start_record: u64,
    chunk_end_record: u64,
) {
    if !is_enabled() {
        return;
    }

    CURRENT_BASE_RECORD_OFFSET.with(|value| value.set(base_offset));
    CURRENT_CHUNK_START_RECORD.with(|value| value.set(Some(chunk_start_record)));
    CURRENT_CHUNK_END_RECORD.with(|value| value.set(Some(chunk_end_record)));
}

pub(crate) fn enter_extension_load_scope() -> ExtensionLoadGuard {
    if is_enabled() {
        EXTENSION_LOAD_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        ExtensionLoadGuard { active: true }
    } else {
        ExtensionLoadGuard { active: false }
    }
}

pub(crate) fn is_extension_load_active() -> bool {
    if !is_enabled() {
        return false;
    }
    EXTENSION_LOAD_DEPTH.with(|depth| depth.get() != 0)
}

pub(crate) fn record_scanned_record() {
    if is_enabled() {
        RECORDS_SCANNED.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_attr_list_present(attr_list: &AttributeListInfo) {
    if !is_enabled() {
        return;
    }

    RECORDS_WITH_ATTR_LIST.fetch_add(1, Ordering::Relaxed);
    match attr_list {
        AttributeListInfo::Resident(bytes) => {
            RESIDENT_ATTR_LISTS.fetch_add(1, Ordering::Relaxed);
            RESIDENT_ATTR_LIST_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        AttributeListInfo::NonResident { data_size, .. } => {
            NONRESIDENT_ATTR_LISTS.fetch_add(1, Ordering::Relaxed);
            NONRESIDENT_ATTR_LIST_BYTES.fetch_add(*data_size, Ordering::Relaxed);
        }
    }
}

pub(crate) fn record_need_check(needs_enrich: bool) {
    if !is_enabled() {
        return;
    }

    if needs_enrich {
        RECORDS_NEEDING_ENRICH.fetch_add(1, Ordering::Relaxed);
    } else {
        RECORDS_SKIPPED_AFTER_NEED_CHECK.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_enrichment(stats: AttrListEnrichStats, elapsed: Duration) {
    if !is_enabled() {
        return;
    }

    record_enrichment_counts(stats);
    record_enrichment_wall_time(elapsed);
}

pub(crate) fn record_enrichment_counts(stats: AttrListEnrichStats) {
    if !is_enabled() {
        return;
    }

    ENRICHMENT_CALLS.fetch_add(1, Ordering::Relaxed);
    EXTENSION_RECORDS_REFERENCED.fetch_add(stats.extension_records_referenced, Ordering::Relaxed);
    EXTENSION_RECORDS_LOADED.fetch_add(stats.extension_records_loaded, Ordering::Relaxed);
}

pub(crate) fn record_enrichment_wall_time(elapsed: Duration) {
    add_duration(&ENRICH_WALL_TIME_NANOS, elapsed);
}

pub(crate) fn record_attr_list_materialize_time(elapsed: Duration) {
    add_duration(&ATTR_LIST_MATERIALIZE_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_record_discovery_time(elapsed: Duration) {
    add_duration(&EXTENSION_RECORD_DISCOVERY_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_record_load_attempt(elapsed: Duration) {
    if !is_enabled() {
        return;
    }

    EXTENSION_RECORD_LOAD_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    add_duration(&EXTENSION_RECORD_LOAD_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_record_target(record_number: u64, offset: u64, record_size: u64) {
    if !is_enabled() {
        return;
    }

    if let Ok(mut counts) = extension_record_load_counts().lock() {
        counts
            .entry(record_number)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
    }

    LAST_EXTENSION_LOAD_OFFSET.with(|last_offset| {
        if let Some(previous) = last_offset.get() {
            let abs_jump = offset.abs_diff(previous);
            EXTENSION_OFFSET_SEQUENCE_SAMPLES.fetch_add(1, Ordering::Relaxed);
            EXTENSION_OFFSET_ABS_JUMP_BYTES.fetch_add(abs_jump, Ordering::Relaxed);
            update_max(&EXTENSION_OFFSET_MAX_ABS_JUMP_BYTES, abs_jump);
            if abs_jump == record_size {
                EXTENSION_OFFSET_EXACT_ADJACENT.fetch_add(1, Ordering::Relaxed);
            }
            if offset >= previous {
                EXTENSION_OFFSET_FORWARD.fetch_add(1, Ordering::Relaxed);
            } else {
                EXTENSION_OFFSET_BACKWARD.fetch_add(1, Ordering::Relaxed);
            }
            if abs_jump <= (1 << 20) {
                EXTENSION_OFFSET_JUMP_LE_1_MIB.fetch_add(1, Ordering::Relaxed);
            }
            if abs_jump <= (8 << 20) {
                EXTENSION_OFFSET_JUMP_LE_8_MIB.fetch_add(1, Ordering::Relaxed);
            }
            if abs_jump > (64 << 20) {
                EXTENSION_OFFSET_JUMP_GT_64_MIB.fetch_add(1, Ordering::Relaxed);
            }
        }
        last_offset.set(Some(offset));
    });

    CURRENT_BASE_RECORD_OFFSET.with(|base_offset| {
        if let Some(base_offset) = base_offset.get() {
            let abs_jump = offset.abs_diff(base_offset);
            BASE_TO_EXTENSION_SAMPLES.fetch_add(1, Ordering::Relaxed);
            BASE_TO_EXTENSION_ABS_JUMP_BYTES.fetch_add(abs_jump, Ordering::Relaxed);
            update_max(&BASE_TO_EXTENSION_MAX_ABS_JUMP_BYTES, abs_jump);
            if abs_jump <= (1 << 20) {
                BASE_TO_EXTENSION_JUMP_LE_1_MIB.fetch_add(1, Ordering::Relaxed);
            }
            if abs_jump <= (8 << 20) {
                BASE_TO_EXTENSION_JUMP_LE_8_MIB.fetch_add(1, Ordering::Relaxed);
            }
            if abs_jump > (64 << 20) {
                BASE_TO_EXTENSION_JUMP_GT_64_MIB.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    CURRENT_CHUNK_START_RECORD.with(|chunk_start| {
        CURRENT_CHUNK_END_RECORD.with(|chunk_end| {
            if let (Some(start), Some(end)) = (chunk_start.get(), chunk_end.get()) {
                if record_number >= start && record_number < end {
                    EXTENSION_WITHIN_CURRENT_CHUNK.fetch_add(1, Ordering::Relaxed);
                } else {
                    EXTENSION_OUTSIDE_CURRENT_CHUNK.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    });
}

pub(crate) fn record_extension_offset_lookup_time(elapsed: Duration) {
    add_duration(&EXTENSION_OFFSET_LOOKUP_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_record_read_time(elapsed: Duration) {
    add_duration(&EXTENSION_RECORD_READ_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_record_parse_time(elapsed: Duration) {
    add_duration(&EXTENSION_RECORD_PARSE_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_entry_build_time(elapsed: Duration) {
    add_duration(&EXTENSION_ENTRY_BUILD_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_record_visit_time(elapsed: Duration) {
    add_duration(&EXTENSION_RECORD_VISIT_TIME_NANOS, elapsed);
}

pub(crate) fn record_extension_file_name_materialized(code_units: usize, elapsed: Duration) {
    if !is_enabled() {
        return;
    }

    EXTENSION_FILE_NAME_MATERIALIZE_CALLS.fetch_add(1, Ordering::Relaxed);
    EXTENSION_FILE_NAME_CODE_UNITS.fetch_add(code_units as u64, Ordering::Relaxed);
    add_duration(&EXTENSION_FILE_NAME_MATERIALIZE_TIME_NANOS, elapsed);
}

pub(crate) fn record_link_merge_shape(
    output_links: usize,
    inline_name_copies: usize,
    slice_copy_inputs: usize,
) {
    if !is_enabled() {
        return;
    }

    LINK_MERGE_CALLS.fetch_add(1, Ordering::Relaxed);
    LINK_MERGE_OUTPUT_LINKS.fetch_add(output_links as u64, Ordering::Relaxed);
    LINK_INLINE_NAME_COPIES.fetch_add(inline_name_copies as u64, Ordering::Relaxed);
    LINK_SLICE_COPY_INPUTS.fetch_add(slice_copy_inputs as u64, Ordering::Relaxed);
}

pub(crate) fn record_link_merge_time(elapsed: Duration) {
    add_duration(&LINK_MERGE_TIME_NANOS, elapsed);
}

pub(crate) fn record_data_merge_time(elapsed: Duration) {
    add_duration(&DATA_MERGE_TIME_NANOS, elapsed);
}

pub(crate) fn record_selected_name_replace_time(elapsed: Duration) {
    if !is_enabled() {
        return;
    }

    SELECTED_NAME_REPLACEMENTS.fetch_add(1, Ordering::Relaxed);
    add_duration(&SELECTED_NAME_REPLACE_TIME_NANOS, elapsed);
}

fn reset() {
    RECORDS_SCANNED.store(0, Ordering::Relaxed);
    RECORDS_WITH_ATTR_LIST.store(0, Ordering::Relaxed);
    RESIDENT_ATTR_LISTS.store(0, Ordering::Relaxed);
    NONRESIDENT_ATTR_LISTS.store(0, Ordering::Relaxed);
    RECORDS_NEEDING_ENRICH.store(0, Ordering::Relaxed);
    RECORDS_SKIPPED_AFTER_NEED_CHECK.store(0, Ordering::Relaxed);
    ENRICHMENT_CALLS.store(0, Ordering::Relaxed);
    EXTENSION_RECORDS_REFERENCED.store(0, Ordering::Relaxed);
    EXTENSION_RECORDS_LOADED.store(0, Ordering::Relaxed);
    RESIDENT_ATTR_LIST_BYTES.store(0, Ordering::Relaxed);
    NONRESIDENT_ATTR_LIST_BYTES.store(0, Ordering::Relaxed);
    ENRICH_WALL_TIME_NANOS.store(0, Ordering::Relaxed);
    ATTR_LIST_MATERIALIZE_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_RECORD_DISCOVERY_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_RECORD_LOAD_ATTEMPTS.store(0, Ordering::Relaxed);
    EXTENSION_RECORD_LOAD_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_LOOKUP_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_RECORD_READ_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_RECORD_PARSE_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_ENTRY_BUILD_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_RECORD_VISIT_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_FILE_NAME_MATERIALIZE_CALLS.store(0, Ordering::Relaxed);
    EXTENSION_FILE_NAME_CODE_UNITS.store(0, Ordering::Relaxed);
    EXTENSION_FILE_NAME_MATERIALIZE_TIME_NANOS.store(0, Ordering::Relaxed);
    LINK_MERGE_CALLS.store(0, Ordering::Relaxed);
    LINK_MERGE_OUTPUT_LINKS.store(0, Ordering::Relaxed);
    LINK_INLINE_NAME_COPIES.store(0, Ordering::Relaxed);
    LINK_SLICE_COPY_INPUTS.store(0, Ordering::Relaxed);
    LINK_MERGE_TIME_NANOS.store(0, Ordering::Relaxed);
    DATA_MERGE_TIME_NANOS.store(0, Ordering::Relaxed);
    SELECTED_NAME_REPLACEMENTS.store(0, Ordering::Relaxed);
    SELECTED_NAME_REPLACE_TIME_NANOS.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_SEQUENCE_SAMPLES.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_EXACT_ADJACENT.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_FORWARD.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_BACKWARD.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_JUMP_LE_1_MIB.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_JUMP_LE_8_MIB.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_JUMP_GT_64_MIB.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_ABS_JUMP_BYTES.store(0, Ordering::Relaxed);
    EXTENSION_OFFSET_MAX_ABS_JUMP_BYTES.store(0, Ordering::Relaxed);
    BASE_TO_EXTENSION_SAMPLES.store(0, Ordering::Relaxed);
    BASE_TO_EXTENSION_JUMP_LE_1_MIB.store(0, Ordering::Relaxed);
    BASE_TO_EXTENSION_JUMP_LE_8_MIB.store(0, Ordering::Relaxed);
    BASE_TO_EXTENSION_JUMP_GT_64_MIB.store(0, Ordering::Relaxed);
    BASE_TO_EXTENSION_ABS_JUMP_BYTES.store(0, Ordering::Relaxed);
    BASE_TO_EXTENSION_MAX_ABS_JUMP_BYTES.store(0, Ordering::Relaxed);
    EXTENSION_WITHIN_CURRENT_CHUNK.store(0, Ordering::Relaxed);
    EXTENSION_OUTSIDE_CURRENT_CHUNK.store(0, Ordering::Relaxed);
    EXTENSION_LOAD_DEPTH.with(|depth| depth.set(0));
    LAST_EXTENSION_LOAD_OFFSET.with(|last_offset| last_offset.set(None));
    CURRENT_BASE_RECORD_OFFSET.with(|value| value.set(None));
    CURRENT_CHUNK_START_RECORD.with(|value| value.set(None));
    CURRENT_CHUNK_END_RECORD.with(|value| value.set(None));
    if let Ok(mut counts) = extension_record_load_counts().lock() {
        counts.clear();
    }
}

fn add_duration(target: &AtomicU64, elapsed: Duration) {
    if is_enabled() {
        target.fetch_add(
            elapsed.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

fn extension_record_load_counts() -> &'static Mutex<FxHashMap<u64, u32>> {
    EXTENSION_RECORD_LOAD_COUNTS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn update_max(target: &AtomicU64, candidate: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use super::*;

    fn profile_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn collects_resident_and_nonresident_attr_list_counters() {
        let _lock = profile_test_lock()
            .lock()
            .expect("profile test lock should not poison");
        let _guard = start();
        record_scanned_record();
        record_scanned_record();
        record_attr_list_present(&AttributeListInfo::Resident(vec![1, 2, 3, 4]));
        record_need_check(false);
        record_attr_list_present(&AttributeListInfo::NonResident {
            runs_data: vec![0x11, 0x22],
            data_size: 4096,
        });
        record_need_check(true);
        record_attr_list_materialize_time(Duration::from_micros(10));
        record_extension_record_discovery_time(Duration::from_micros(20));
        record_extension_record_load_attempt(Duration::from_micros(30));
        record_extension_record_load_attempt(Duration::from_micros(30));
        record_extension_record_load_attempt(Duration::from_micros(30));
        set_current_enrichment_base_context(Some(9_500), 10, 12);
        record_extension_record_target(11, 10_000, 1024);
        record_extension_record_target(11, 11_024, 1024);
        record_extension_record_target(12, 100_000_000, 1024);
        record_extension_offset_lookup_time(Duration::from_micros(31));
        record_extension_record_read_time(Duration::from_micros(32));
        record_extension_record_parse_time(Duration::from_micros(33));
        record_extension_entry_build_time(Duration::from_micros(34));
        record_extension_record_visit_time(Duration::from_micros(40));
        let _scope = enter_extension_load_scope();
        record_extension_file_name_materialized(12, Duration::from_micros(50));
        record_link_merge_shape(4, 2, 3);
        record_link_merge_time(Duration::from_micros(60));
        record_data_merge_time(Duration::from_micros(70));
        record_selected_name_replace_time(Duration::from_micros(80));
        record_enrichment(
            AttrListEnrichStats {
                extension_records_referenced: 3,
                extension_records_loaded: 2,
            },
            Duration::from_micros(250),
        );

        let snapshot = snapshot();
        assert_eq!(snapshot.records_scanned, 2);
        assert_eq!(snapshot.records_with_attr_list, 2);
        assert_eq!(snapshot.resident_attr_lists, 1);
        assert_eq!(snapshot.nonresident_attr_lists, 1);
        assert_eq!(snapshot.records_needing_enrich, 1);
        assert_eq!(snapshot.records_skipped_after_need_check, 1);
        assert_eq!(snapshot.enrichment_calls, 1);
        assert_eq!(snapshot.extension_records_referenced, 3);
        assert_eq!(snapshot.extension_records_loaded, 2);
        assert_eq!(snapshot.resident_attr_list_bytes, 4);
        assert_eq!(snapshot.nonresident_attr_list_bytes, 4096);
        assert_eq!(
            snapshot.attr_list_materialize_time,
            Duration::from_micros(10)
        );
        assert_eq!(
            snapshot.extension_record_discovery_time,
            Duration::from_micros(20)
        );
        assert_eq!(snapshot.extension_record_load_attempts, 3);
        assert_eq!(
            snapshot.extension_record_load_time,
            Duration::from_micros(90)
        );
        assert_eq!(
            snapshot.extension_offset_lookup_time,
            Duration::from_micros(31)
        );
        assert_eq!(
            snapshot.extension_record_read_time,
            Duration::from_micros(32)
        );
        assert_eq!(
            snapshot.extension_record_parse_time,
            Duration::from_micros(33)
        );
        assert_eq!(
            snapshot.extension_entry_build_time,
            Duration::from_micros(34)
        );
        assert_eq!(
            snapshot.extension_record_visit_time,
            Duration::from_micros(40)
        );
        assert_eq!(snapshot.extension_file_name_materialize_calls, 1);
        assert_eq!(snapshot.extension_file_name_code_units, 12);
        assert_eq!(
            snapshot.extension_file_name_materialize_time,
            Duration::from_micros(50)
        );
        assert_eq!(snapshot.link_merge_calls, 1);
        assert_eq!(snapshot.link_merge_output_links, 4);
        assert_eq!(snapshot.link_inline_name_copies, 2);
        assert_eq!(snapshot.link_slice_copy_inputs, 3);
        assert_eq!(snapshot.link_merge_time, Duration::from_micros(60));
        assert_eq!(snapshot.data_merge_time, Duration::from_micros(70));
        assert_eq!(snapshot.selected_name_replacements, 1);
        assert_eq!(
            snapshot.selected_name_replace_time,
            Duration::from_micros(80)
        );
        assert_eq!(snapshot.unique_extension_records_loaded, 2);
        assert_eq!(snapshot.records_reloaded, 1);
        assert_eq!(snapshot.repeated_extension_loads, 1);
        assert_eq!(snapshot.max_loads_for_single_extension_record, 2);
        assert_eq!(snapshot.extension_offset_sequence_samples, 2);
        assert_eq!(snapshot.extension_offset_exact_adjacent, 1);
        assert_eq!(snapshot.extension_offset_forward, 2);
        assert_eq!(snapshot.extension_offset_backward, 0);
        assert_eq!(snapshot.extension_offset_jump_le_1_mib, 1);
        assert_eq!(snapshot.extension_offset_jump_le_8_mib, 1);
        assert_eq!(snapshot.extension_offset_jump_gt_64_mib, 1);
        assert_eq!(snapshot.extension_offset_max_abs_jump_bytes, 99_988_976);
        assert_eq!(snapshot.base_to_extension_samples, 3);
        assert_eq!(snapshot.base_to_extension_jump_le_1_mib, 2);
        assert_eq!(snapshot.base_to_extension_jump_le_8_mib, 2);
        assert_eq!(snapshot.base_to_extension_jump_gt_64_mib, 1);
        assert_eq!(snapshot.base_to_extension_max_abs_jump_bytes, 99_990_500);
        assert_eq!(snapshot.extension_within_current_chunk, 2);
        assert_eq!(snapshot.extension_outside_current_chunk, 1);
        assert_eq!(snapshot.enrich_wall_time, Duration::from_micros(250));
    }

    #[test]
    fn resets_counters_between_runs() {
        let _lock = profile_test_lock()
            .lock()
            .expect("profile test lock should not poison");
        let _guard = start();
        record_scanned_record();
        assert_eq!(snapshot().records_scanned, 1);
        drop(_guard);

        let _guard = start();
        assert_eq!(snapshot(), AttrListProfileSnapshot::default());
    }
}
