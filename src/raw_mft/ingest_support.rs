//! Shared helpers for the raw-MFT parallel ingest benchmark and profiling example.

use std::{
    env,
    ffi::OsStr,
    num::{NonZeroU64, NonZeroUsize},
    sync::OnceLock,
    thread,
};

use rustc_hash::FxHashMap;

use crate::{
    Fid,
    errors::UsnError,
    volume::Volume,
};

use super::{
    FileNameNamespace, RawMft, RawMftBatchEntry, RawMftChunkPlanOptions, RawMftEntry,
    RawMftLink, RawMftScanOptions, RawMftWorkChunk,
};

/// Default main read buffer size for the parallel ingest path.
const DEFAULT_MAIN_BUFFER_BYTES: usize = 512 * 1024;
/// Default attribute-list read buffer size for the parallel ingest path.
const DEFAULT_ATTR_BUFFER_BYTES: usize = 16 * 1024;
/// Default number of logical records per chunk.
const DEFAULT_CHUNK_RECORDS: u64 = 16 * 1024;
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
        }
    }

    /// Build scan options for the serial ingest path.
    fn iter_options(&self) -> RawMftScanOptions {
        RawMftScanOptions::builder()
            .buffer_bytes(self.main_buffer_bytes)
            .attr_buffer_bytes(self.attr_buffer_bytes)
            .skip_unused(true)
            .skip_extension_records(true)
            .collect_alternate_data_streams(false)
            .collect_data_run_summary(false)
            .collect_dos_file_name_links(false)
            .start_record(self.start_record)
            .end_record(self.end_record)
            .build()
    }

    /// Build chunk-planning options for the parallel ingest path.
    fn chunk_plan_options(&self) -> RawMftChunkPlanOptions {
        RawMftChunkPlanOptions::builder()
            .skip_unused(false)
            .start_record(self.start_record)
            .end_record(self.end_record)
            .max_records_per_chunk(self.chunk_records)
            .build()
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
}

/// Return whether the serial ingest benchmark should also be run.
pub fn include_serial_bench() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_INCLUDE_SERIAL").is_some()
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

/// Estimate the record-table size for the configured scan range.
fn record_count_hint(mft: &RawMft<'_>, config: &BenchConfig) -> usize {
    let total = mft.record_count();
    let end = config.end_record.unwrap_or(total).min(total);
    end.min(usize::MAX as u64) as usize
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
        size: if entry.is_directory { 0 } else { entry.real_size },
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
        size: if entry.is_directory { 0 } else { entry.real_size },
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
    let mut emitted_any = false;
    for (index, link) in all_links.iter().enumerate() {
        if !is_visible_link_at(index, all_links) {
            continue;
        }
        emitted_any = true;
        visit(link.parent_reference, link.file_name.as_os_str());
    }

    if !emitted_any && !file_name.is_empty() {
        visit(parent_reference, file_name);
    }
}

/// Return whether the link at `index` should be emitted.
fn is_visible_link_at(index: usize, all_links: &[RawMftLink]) -> bool {
    let Some(link) = all_links.get(index) else {
        return false;
    };
    if !link_namespace_is_visible(link, all_links) {
        return false;
    }
    !all_links[..index].iter().any(|previous| {
        link_namespace_is_visible(previous, all_links)
            && previous.parent_reference == link.parent_reference
            && previous.file_name == link.file_name
    })
}

/// Return whether a link namespace is visible for the current benchmark output.
fn link_namespace_is_visible(link: &RawMftLink, all_links: &[RawMftLink]) -> bool {
    if link.file_name.is_empty() {
        return false;
    }
    if link.namespace == FileNameNamespace::Posix
        && all_links.iter().any(|other| {
            other.parent_reference == link.parent_reference
                && matches!(
                    other.namespace,
                    FileNameNamespace::Win32 | FileNameNamespace::Win32AndDos
                )
        })
    {
        return false;
    }
    if link.namespace == FileNameNamespace::Dos
        && all_links.iter().any(|other| {
            other.parent_reference == link.parent_reference
                && other.namespace != FileNameNamespace::Dos
        })
    {
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

/// Convert a `usize` into a non-zero `usize`, clamping zero to one.
fn nonzero_usize(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or(NonZeroUsize::MIN)
}

/// Convert a `u64` into a non-zero `u64`, clamping zero to one.
fn nonzero_u64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN)
}


