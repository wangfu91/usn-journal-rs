//! Enumerate deleted / unused raw `$MFT` records and print a best-effort path.
//!
//! This example scans the raw `$MFT` with `include_unused_records(true)` so it
//! can see slots whose `$BITMAP` bit is clear, then builds a snapshot-local
//! path index from all named base records.
//!
//! Resolution quality tags:
//! - `exact`: every component matched the exact `Fid` stored in the record.
//! - `record-fallback`: at least one parent step had to ignore stale sequence
//!   bits and use the current snapshot occupant of that record number.
//! - `partial`: the chain broke before reaching the root, so the printed path
//!   includes an unresolved-parent marker.

use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use usn_journal_rs::{
    Fid,
    errors::UsnError,
    raw_mft::{RawMft, RawMftEntry, RawMftScanOptions},
    volume::Volume,
};

const DEFAULT_LIMIT: usize = 1_000;
const NTFS_ROOT_RECORD_NUMBER: u64 = 5;
const MAX_STEPS: usize = 256;

#[derive(Debug, Clone)]
struct SnapshotNode {
    parent: Fid,
    name: OsString,
}

#[derive(Debug, Clone)]
struct DeletedRecord {
    record_number: u64,
    sequence_number: u16,
    fid: Fid,
    file_name: OsString,
    is_directory: bool,
}

impl DeletedRecord {
    fn from_entry(entry: &RawMftEntry) -> Self {
        Self {
            record_number: entry.record_number,
            sequence_number: entry.sequence_number,
            fid: entry.file_reference,
            file_name: entry.file_name.clone(),
            is_directory: entry.is_directory,
        }
    }

    fn display_name(&self) -> &OsStr {
        if self.file_name.is_empty() {
            OsStr::new("<unnamed>")
        } else {
            self.file_name.as_os_str()
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ResolutionQuality {
    Exact,
    RecordFallback,
    Partial,
}

impl ResolutionQuality {
    fn label(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::RecordFallback => "record-fallback",
            Self::Partial => "partial",
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedPath {
    path: PathBuf,
    quality: ResolutionQuality,
}

#[derive(Debug, Clone)]
struct WalkOutcome {
    components: Vec<OsString>,
    complete: bool,
    missing_parent: Option<Fid>,
}

#[derive(Debug, Default)]
struct HistoricalPathIndex {
    nodes_by_fid: HashMap<Fid, SnapshotNode>,
    fid_by_record: HashMap<u64, Fid>,
}

impl HistoricalPathIndex {
    fn insert(&mut self, entry: &RawMftEntry) {
        if entry.file_name.is_empty() {
            return;
        }

        let Some(record_number) = entry.file_reference.record_number() else {
            return;
        };

        self.fid_by_record.insert(record_number, entry.file_reference);
        self.nodes_by_fid.insert(
            entry.file_reference,
            SnapshotNode {
                parent: entry.parent_reference,
                name: entry.file_name.clone(),
            },
        );
    }

    fn resolve_best_effort(&self, volume: &Volume, record: &DeletedRecord) -> ResolvedPath {
        let exact = self.walk(record.fid, false);
        if exact.complete {
            return ResolvedPath {
                path: compose_path(volume, None, &exact.components, record.display_name()),
                quality: ResolutionQuality::Exact,
            };
        }

        let fallback = self.walk(record.fid, true);
        if fallback.complete {
            return ResolvedPath {
                path: compose_path(volume, None, &fallback.components, record.display_name()),
                quality: ResolutionQuality::RecordFallback,
            };
        }

        let marker = fallback
            .missing_parent
            .map(|fid| format!("<unresolved-parent:{fid}>"));
        ResolvedPath {
            path: compose_path(
                volume,
                marker.as_deref(),
                &fallback.components,
                record.display_name(),
            ),
            quality: ResolutionQuality::Partial,
        }
    }

    fn walk(&self, fid: Fid, allow_record_fallback: bool) -> WalkOutcome {
        let mut components = Vec::with_capacity(16);
        let mut visited = HashSet::with_capacity(32);
        let mut current_fid = fid;
        let mut missing_parent = None;

        for _ in 0..MAX_STEPS {
            let Some(record_number) = current_fid.record_number() else {
                missing_parent = Some(current_fid);
                break;
            };

            if !visited.insert(record_number) {
                missing_parent = Some(current_fid);
                break;
            }

            let node = match self.nodes_by_fid.get(&current_fid) {
                Some(node) => node,
                None if allow_record_fallback => {
                    let Some(fallback_fid) = self.fid_by_record.get(&record_number) else {
                        missing_parent = Some(current_fid);
                        break;
                    };
                    let Some(node) = self.nodes_by_fid.get(fallback_fid) else {
                        missing_parent = Some(current_fid);
                        break;
                    };
                    node
                }
                None => {
                    missing_parent = Some(current_fid);
                    break;
                }
            };

            components.push(node.name.clone());

            let Some(parent_record) = node.parent.record_number() else {
                missing_parent = Some(node.parent);
                break;
            };

            if parent_record == NTFS_ROOT_RECORD_NUMBER || parent_record == record_number {
                return WalkOutcome {
                    components,
                    complete: true,
                    missing_parent: None,
                };
            }

            current_fid = node.parent;
        }

        WalkOutcome {
            components,
            complete: false,
            missing_parent,
        }
    }
}

fn compose_path(
    volume: &Volume,
    marker: Option<&str>,
    components: &[OsString],
    fallback_leaf: &OsStr,
) -> PathBuf {
    let mut path = PathBuf::new();
    if let Some(drive) = volume.drive_letter() {
        path.push(format!("{}:\\", drive.to_ascii_uppercase()));
    } else if let Some(mount_point) = volume.mount_point() {
        path.push(mount_point);
    }

    if let Some(marker) = marker {
        path.push(marker);
    }

    if components.is_empty() {
        path.push(fallback_leaf);
        return path;
    }

    for component in components.iter().rev() {
        path.push(component);
    }
    path
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = env::args()
        .nth(1)
        .and_then(|value| value.chars().next())
        .unwrap_or('C')
        .to_ascii_uppercase();
    let limit = env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIMIT);

    let volume = Volume::from_drive_letter(drive_letter)?;
    let mft = RawMft::new(&volume)?;
    let options = RawMftScanOptions::builder()
        .include_unused_records(true)
        .collect_alternate_data_streams(false)
        .collect_data_run_summary(false)
        .collect_dos_file_name_links(false)
        .build();

    let mut index = HistoricalPathIndex::default();
    let mut deleted_records = Vec::new();

    for result in mft.try_iter_with_options(options)? {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("error: {error}");
                continue;
            }
        };

        index.insert(&entry);
        if !entry.is_used {
            deleted_records.push(DeletedRecord::from_entry(&entry));
        }
    }

    println!(
        "unused raw MFT records on {drive_letter}: ({} total, showing up to {})",
        deleted_records.len(),
        limit
    );

    for record in deleted_records.iter().take(limit) {
        let resolved = index.resolve_best_effort(&volume, record);
        let kind = if record.is_directory { "DIR " } else { "FILE" };
        println!(
            "{kind} #{:>10} seq={:>5} quality={:<15} {}",
            record.record_number,
            record.sequence_number,
            resolved.quality.label(),
            resolved.path.display(),
        );
    }

    if deleted_records.len() > limit {
        eprintln!(
            "truncated output at {limit} records; rerun with a higher limit to inspect more"
        );
    }

    Ok(())
}