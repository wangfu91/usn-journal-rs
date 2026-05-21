//! Experimental historical path reconstruction helpers for raw `$MFT` snapshots.
//!
//! These helpers index one raw-`$MFT` snapshot and intentionally keep their
//! best-effort semantics separate from [`crate::path::PathResolver`]. They can
//! use deleted and stale records from the snapshot, report whether a path was
//! resolved exactly or only by record-number fallback, and preserve partial
//! results when the parent chain breaks.

use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fmt,
    path::PathBuf,
};

use crate::{Fid, raw_mft::RawMftEntry, volume::Volume};

/// NTFS record number reserved for the volume root directory.
const NTFS_ROOT_RECORD_NUMBER: u64 = 5;
/// Maximum parent hops allowed before treating the chain as broken.
const MAX_STEPS: usize = 256;

/// Snapshot-local metadata for one named raw-`$MFT` entry.
#[derive(Debug, Clone)]
struct SnapshotNode {
    /// Parent directory reference recorded for this entry.
    parent: Fid,
    /// File name selected for this snapshot node.
    name: OsString,
}

/// Internal result of walking a parent chain through the snapshot.
#[derive(Debug, Clone)]
struct WalkOutcome {
    /// Path components collected from leaf to root.
    components: Vec<OsString>,
    /// Whether the walk reached the NTFS root without breaking.
    complete: bool,
    /// Parent reference where the walk stopped, if any.
    missing_parent: Option<Fid>,
}

/// Resolution quality for [`HistoricalPathResolution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoricalPathResolutionQuality {
    /// Every path component matched the exact `Fid` stored in the snapshot.
    Exact,
    /// At least one parent step had to fall back to the current snapshot
    /// occupant of the referenced record number.
    RecordFallback,
    /// The parent chain could not be completed to the NTFS root.
    Partial,
}

impl HistoricalPathResolutionQuality {
    /// Stable string label for logs and CLI output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::RecordFallback => "record-fallback",
            Self::Partial => "partial",
        }
    }
}

impl fmt::Display for HistoricalPathResolutionQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Best-effort historical path resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalPathResolution {
    /// Full path assembled from the snapshot and the volume prefix.
    pub path: PathBuf,
    /// How trustworthy the reconstructed path is.
    pub quality: HistoricalPathResolutionQuality,
    /// Parent file reference where the walk broke, when the resolution is only partial.
    pub missing_parent: Option<Fid>,
}

/// Snapshot-local path index for raw-`$MFT` historical reconstruction.
///
/// This type is experimental and is intended for deleted-record and historical
/// path reconstruction from a single raw-`$MFT` snapshot. Insert every named
/// base record you care about, including deleted records when available, then
/// call one of the `resolve_*_best_effort` methods to reconstruct a path.
#[derive(Debug, Default, Clone)]
pub struct HistoricalPathIndex {
    /// Named snapshot nodes keyed by their full file reference.
    nodes_by_fid: HashMap<Fid, SnapshotNode>,
    /// Current snapshot occupants keyed by record number for fallback lookup.
    fid_by_record: HashMap<u64, Fid>,
}

impl HistoricalPathIndex {
    /// Create an empty historical path index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of named snapshot nodes currently indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes_by_fid.len()
    }

    /// Returns `true` when the index contains no named entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes_by_fid.is_empty()
    }

    /// Insert one named raw-`$MFT` entry into the snapshot index.
    ///
    /// Entries without a file name are ignored because they cannot contribute
    /// a usable path component.
    pub fn insert(&mut self, entry: &RawMftEntry) {
        if entry.file_name.is_empty() {
            return;
        }

        let Some(record_number) = entry.file_reference.record_number() else {
            return;
        };

        self.fid_by_record
            .insert(record_number, entry.file_reference);
        self.nodes_by_fid.insert(
            entry.file_reference,
            SnapshotNode {
                parent: entry.parent_reference,
                name: entry.file_name.clone(),
            },
        );
    }

    /// Resolve an arbitrary file reference against the snapshot with best-effort fallback.
    ///
    /// `fallback_leaf` is used when the snapshot cannot provide any named path
    /// component for `fid` itself.
    #[must_use]
    pub fn resolve_best_effort(
        &self,
        volume: &Volume,
        fid: Fid,
        fallback_leaf: &OsStr,
    ) -> HistoricalPathResolution {
        let exact = self.walk(fid, false);
        if exact.complete {
            return HistoricalPathResolution {
                path: compose_path(volume, None, &exact.components, fallback_leaf),
                quality: HistoricalPathResolutionQuality::Exact,
                missing_parent: None,
            };
        }

        let fallback = self.walk(fid, true);
        if fallback.complete {
            return HistoricalPathResolution {
                path: compose_path(volume, None, &fallback.components, fallback_leaf),
                quality: HistoricalPathResolutionQuality::RecordFallback,
                missing_parent: None,
            };
        }

        let marker = fallback
            .missing_parent
            .map(|missing_parent| format!("<unresolved-parent:{missing_parent}>"));
        HistoricalPathResolution {
            path: compose_path(
                volume,
                marker.as_deref(),
                &fallback.components,
                fallback_leaf,
            ),
            quality: HistoricalPathResolutionQuality::Partial,
            missing_parent: fallback.missing_parent,
        }
    }

    /// Resolve a raw-`$MFT` entry against the snapshot with best-effort fallback.
    #[must_use]
    pub fn resolve_entry_best_effort(
        &self,
        volume: &Volume,
        entry: &RawMftEntry,
    ) -> HistoricalPathResolution {
        self.resolve_best_effort(volume, entry.file_reference, entry.file_name.as_os_str())
    }

    /// Walk the parent chain for a file reference with optional record-number fallback.
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

/// Assemble a display path from the volume prefix, an optional unresolved marker,
/// and the collected leaf-to-root path components.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FileAttributes, Filetime,
        raw_mft::{FileNameNamespace, RawMftEntry},
        volume::{Volume, VolumeSource},
    };
    use std::{ffi::OsString, path::PathBuf};
    use windows::Win32::Foundation::HANDLE;

    fn standard_fid(record_number: u64, sequence_number: u16) -> Fid {
        Fid::new(((sequence_number as u64) << 48) | record_number)
    }

    fn sample_entry(
        record_number: u64,
        sequence_number: u16,
        parent_record: u64,
        parent_sequence: u16,
        file_name: &str,
    ) -> RawMftEntry {
        RawMftEntry {
            record_number,
            sequence_number,
            file_reference: standard_fid(record_number, sequence_number),
            parent_reference: standard_fid(parent_record, parent_sequence),
            base_record_reference: 0,
            hard_link_count: 1,
            flags: 0,
            is_used: true,
            is_directory: false,
            is_reparse_point: false,
            reparse_tag: None,
            namespace: FileNameNamespace::Win32,
            file_name: OsString::from(file_name),
            si_created: Filetime::new(0),
            si_modified: Filetime::new(0),
            si_mft_modified: Filetime::new(0),
            si_accessed: Filetime::new(0),
            si_file_attributes: FileAttributes::empty(),
            fn_created: Filetime::new(0),
            fn_modified: Filetime::new(0),
            fn_mft_modified: Filetime::new(0),
            fn_accessed: Filetime::new(0),
            real_size: 0,
            allocated_size: 0,
            has_unnamed_data: false,
            is_resident: true,
            is_sparse: false,
            is_compressed: false,
            is_encrypted: false,
            data_run_summary: None,
            alternate_data_streams: Box::default(),
            links: Box::default(),
        }
    }

    fn mock_volume() -> Volume {
        Volume::mock(HANDLE(std::ptr::null_mut()), VolumeSource::DriveLetter('C'))
    }

    #[test]
    fn resolves_exact_snapshot_path() {
        let mut index = HistoricalPathIndex::new();
        let directory = sample_entry(7, 2, NTFS_ROOT_RECORD_NUMBER, 1, "dir");
        let file = sample_entry(10, 3, 7, 2, "file.txt");
        index.insert(&directory);
        index.insert(&file);

        let resolved = index.resolve_entry_best_effort(&mock_volume(), &file);

        assert_eq!(resolved.quality, HistoricalPathResolutionQuality::Exact);
        assert_eq!(resolved.missing_parent, None);
        assert_eq!(resolved.path, PathBuf::from(r"C:\dir\file.txt"));
    }

    #[test]
    fn falls_back_to_current_record_occupant_when_sequence_mismatches() {
        let mut index = HistoricalPathIndex::new();
        let current_parent = sample_entry(7, 9, NTFS_ROOT_RECORD_NUMBER, 1, "dir");
        let file = sample_entry(10, 3, 7, 2, "file.txt");
        index.insert(&current_parent);
        index.insert(&file);

        let resolved = index.resolve_entry_best_effort(&mock_volume(), &file);

        assert_eq!(
            resolved.quality,
            HistoricalPathResolutionQuality::RecordFallback
        );
        assert_eq!(resolved.missing_parent, None);
        assert_eq!(resolved.path, PathBuf::from(r"C:\dir\file.txt"));
    }

    #[test]
    fn returns_partial_resolution_when_parent_chain_breaks() {
        let mut index = HistoricalPathIndex::new();
        let file = sample_entry(10, 3, 99, 2, "file.txt");
        index.insert(&file);

        let resolved = index.resolve_entry_best_effort(&mock_volume(), &file);

        assert_eq!(resolved.quality, HistoricalPathResolutionQuality::Partial);
        assert_eq!(resolved.missing_parent, Some(standard_fid(99, 2)));
        assert_eq!(
            resolved.path,
            PathBuf::from(format!(
                r"C:\<unresolved-parent:{}>\file.txt",
                standard_fid(99, 2)
            ))
        );
    }
}
