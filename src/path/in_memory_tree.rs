//! In-memory directory tree built from a raw `$MFT` scan.
//!
//! [`InMemoryDirTree`] is a pre-built, read-only map from 48-bit MFT record
//! numbers to `(parent, UTF-16 name)` pairs. Once constructed, resolving a
//! file path to the root is a pure pointer-chase with no syscalls.

use super::util::{NTFS_ROOT_RECORD_NUMBER, mask_fid_to_record_number};
use crate::{Fid, raw_mft::RawMft};
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};

/// Directory entry in the in-memory tree. Stores the parent file
/// reference number (full 64-bit, not masked) and the leaf name as raw
/// UTF-16 units so we don't pay an `OsString` allocation per entry.
#[derive(Debug, Clone)]
struct DirEntry {
    /// Full parent file reference for this path component.
    parent: Fid,
    /// UTF-16 leaf name stored without allocating an `OsString`.
    name: Box<[u16]>,
}

/// Pre-built in-memory directory tree keyed by 48-bit MFT record number.
///
/// Built in a single pass over the raw `$MFT`. Resolving a path is then
/// a pointer chase up to the root with no syscalls and no `PathBuf`
/// allocations until the final assembly.
#[derive(Debug, Default, Clone)]
pub struct InMemoryDirTree {
    /// Map from 48-bit NTFS record number to its parent/name pair.
    entries: FxHashMap<u64, DirEntry>,
}

impl InMemoryDirTree {
    /// Build the tree from a raw `$MFT` reader. Iterates every record
    /// once. Skips entries marked unused in the `$MFT $BITMAP`.
    pub fn from_raw_mft(raw_mft: &RawMft<'_>) -> crate::UsnResult<Self> {
        let record_count = raw_mft.record_count() as usize;
        let estimated_used_records = if record_count < 2_048 {
            record_count
        } else {
            record_count / 2
        };
        let mut entries =
            FxHashMap::with_capacity_and_hasher(estimated_used_records, Default::default());
        for r in raw_mft.iter()? {
            let entry = match r {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.is_used {
                continue;
            }
            if entry.file_name.is_empty() {
                continue;
            }
            let Some(key) = mask_fid_to_record_number(entry.file_reference) else {
                continue;
            };
            // Encode the file name as raw UTF-16 once and store it.
            let units: Vec<u16> = entry.file_name.encode_wide().collect();
            entries.insert(
                key,
                DirEntry {
                    parent: entry.parent_reference,
                    name: units.into_boxed_slice(),
                },
            );
        }
        entries.shrink_to_fit();
        Ok(InMemoryDirTree { entries })
    }

    /// Number of entries currently stored.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the tree has no entries.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert a directory entry (testing / advanced use).
    #[cfg(test)]
    #[doc(hidden)]
    pub(crate) fn insert(&mut self, fid: u64, parent: u64, name: &[u16]) {
        self.entries.insert(
            Fid::new(fid)
                .record_number()
                .expect("standard fid must expose record number"),
            DirEntry {
                parent: Fid::new(parent),
                name: name.to_vec().into_boxed_slice(),
            },
        );
    }

    /// Walks parents up to the root and returns the resolved path
    /// (without drive prefix). Returns `None` if the chain breaks or a
    /// cycle is detected.
    #[must_use]
    pub fn resolve(&self, fid: Fid) -> Option<PathBuf> {
        self.resolve_with_optional_drive(fid, None)
    }

    /// Walks parents and prepends `<drive>:\` to the resolved path.
    #[must_use]
    pub fn resolve_with_drive_letter(&self, fid: Fid, drive: char) -> Option<PathBuf> {
        self.resolve_with_optional_drive(fid, Some(drive))
    }

    /// Resolve a path and optionally prepend a drive-letter prefix.
    pub(super) fn resolve_with_optional_drive(
        &self,
        fid: Fid,
        drive: Option<char>,
    ) -> Option<PathBuf> {
        // Maximum walk depth: comfortably above practical NTFS path depths
        // while still bounding malformed parent chains.
        const MAX_STEPS: usize = 256;

        let mut chain: Vec<&[u16]> = Vec::with_capacity(32);
        let mut current = mask_fid_to_record_number(fid)?;
        let mut visited = FxHashSet::default();
        let mut steps = 0usize;
        loop {
            if steps >= MAX_STEPS || !visited.insert(current) {
                return None;
            }
            steps += 1;

            let entry = self.entries.get(&current)?;
            chain.push(&entry.name);

            let parent = mask_fid_to_record_number(entry.parent)?;
            // Stop at the NTFS root (`$Root`) or on a self-parenting record.
            if parent == current || parent == NTFS_ROOT_RECORD_NUMBER {
                break;
            }
            current = parent;
        }

        let mut path = PathBuf::new();
        if let Some(drive) = drive {
            let drive = drive.to_ascii_uppercase();
            path.push(format!("{drive}:\\"));
        }
        for units in chain.iter().rev() {
            path.push(OsString::from_wide(units));
        }
        Some(path)
    }
}
