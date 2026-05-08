use std::{
    env,
    ffi::OsStr,
    num::{NonZeroU64, NonZeroUsize},
    sync::OnceLock,
    thread,
};

use rustc_hash::FxHashMap;
use usn_journal_rs::{
    errors::UsnError,
    raw_mft::{
        FileNameNamespace, RawMft, RawMftBatchEntry, RawMftChunkPlanOptions, RawMftEntry,
        RawMftLink, RawMftScanOptions, RawMftWorkChunk,
    },
    volume::Volume,
};

const DEFAULT_MAIN_BUFFER_BYTES: usize = 512 * 1024;
const DEFAULT_ATTR_BUFFER_BYTES: usize = 16 * 1024;
const DEFAULT_CHUNK_RECORDS: u64 = 16 * 1024;
const FIRST_NORMAL_RECORD: u64 = 24;

#[derive(Debug, Clone)]
pub struct BenchConfig {
    pub drive: char,
    pub worker_count: NonZeroUsize,
    pub main_buffer_bytes: NonZeroUsize,
    pub attr_buffer_bytes: NonZeroUsize,
    pub chunk_records: NonZeroU64,
    pub start_record: u64,
    pub end_record: Option<u64>,
}

impl BenchConfig {
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

    fn iter_options(&self) -> RawMftScanOptions {
        RawMftScanOptions::builder()
            .buffer_bytes(self.main_buffer_bytes)
            .attr_buffer_bytes(self.attr_buffer_bytes)
            .skip_extension_records(true)
            .collect_alternate_data_streams(false)
            .collect_data_run_summary(false)
            .collect_dos_file_name_links(false)
            .start_record(self.start_record)
            .end_record(self.end_record)
            .build()
    }

    fn work_plan_options(&self) -> RawMftChunkPlanOptions {
        RawMftChunkPlanOptions::builder()
            .skip_unused(false)
            .start_record(self.start_record)
            .end_record(self.end_record)
            .max_records_per_chunk(self.chunk_records)
            .build()
    }
}

#[derive(Debug, Clone)]
struct BenchNodeMeta {
    size: u64,
    allocated_size: u64,
}

#[derive(Debug, Clone)]
struct BenchChildLink {
    child_record: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BenchSummary {
    pub records: usize,
    pub parent_buckets: usize,
    pub child_links: usize,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
}

struct BenchTargets<'a> {
    records: &'a mut [Option<BenchNodeMeta>],
    children_by_parent: &'a mut FxHashMap<u64, Vec<BenchChildLink>>,
}

struct PartialIngest {
    records: Vec<(u64, BenchNodeMeta)>,
    child_links: Vec<(u64, BenchChildLink)>,
}

pub fn bench_config() -> &'static BenchConfig {
    static CONFIG: OnceLock<BenchConfig> = OnceLock::new();
    CONFIG.get_or_init(BenchConfig::from_env)
}

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

pub fn include_serial_bench() -> bool {
    env::var_os("USN_RAW_MFT_BENCH_INCLUDE_SERIAL").is_some()
}

pub fn run_parallel_ingest(
    mft: &RawMft<'_>,
    config: &BenchConfig,
) -> Result<BenchSummary, UsnError> {
    let chunks = mft.plan_chunks_with_options(config.work_plan_options());
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

fn record_count_hint(mft: &RawMft<'_>, config: &BenchConfig) -> usize {
    let total = mft.record_count();
    let end = config.end_record.unwrap_or(total).min(total);
    end.min(usize::MAX as u64) as usize
}

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

fn new_partial_ingest(chunk: RawMftWorkChunk) -> PartialIngest {
    let capacity = chunk.record_len().min(usize::MAX as u64) as usize;
    PartialIngest {
        records: Vec::with_capacity(capacity),
        child_links: Vec::with_capacity(capacity),
    }
}

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

fn for_each_visible_link<F>(
    parent_reference: usn_journal_rs::Fid,
    _namespace: FileNameNamespace,
    file_name: &OsStr,
    all_links: &[RawMftLink],
    mut visit: F,
) where
    F: FnMut(usn_journal_rs::Fid, &OsStr),
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

fn pick_drive() -> char {
    env::var("USN_RAW_MFT_BENCH_DRIVE")
        .ok()
        .or_else(|| env::var("USN_TEST_DRIVE").ok())
        .and_then(|value| value.chars().next())
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or('C')
}

fn default_worker_count() -> NonZeroUsize {
    let available = thread::available_parallelism()
        .ok()
        .map(NonZeroUsize::get)
        .unwrap_or(1);
    nonzero_usize(available.clamp(1, 10))
}

fn parse_env_nonzero_usize(name: &str, default: NonZeroUsize) -> NonZeroUsize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(NonZeroUsize::new)
        .unwrap_or(default)
}

fn parse_env_nonzero_u64(name: &str, default: NonZeroU64) -> NonZeroU64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(NonZeroU64::new)
        .unwrap_or(default)
}

fn nonzero_usize(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or(NonZeroUsize::MIN)
}

fn nonzero_u64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN)
}
