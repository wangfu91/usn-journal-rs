//! In-memory directory tree built from a raw `$MFT` scan.
//!
//! [`InMemoryDirTree`] is a pre-built, read-only map from 48-bit MFT record
//! numbers to `(parent, UTF-16 name)` pairs. Once constructed, resolving a
//! file path to the root is a pure pointer-chase with no syscalls.

use super::util::{NTFS_ROOT_RECORD_NUMBER, mask_fid_to_record_number};
use crate::{Fid, raw_mft::RawMft};
use rustc_hash::FxHashMap;
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
    parent: Fid,
    name: Box<[u16]>,
}

/// Pre-built in-memory directory tree keyed by 48-bit MFT record number.
///
/// Built in a single pass over the raw `$MFT`. Resolving a path is then
/// a pointer chase up to the root with no syscalls and no `PathBuf`
/// allocations until the final assembly.
#[derive(Debug, Default, Clone)]
pub struct InMemoryDirTree {
    entries: FxHashMap<u64, DirEntry>,
}

impl InMemoryDirTree {
    /// Build the tree from a raw `$MFT` reader. Iterates every record
    /// once. Skips entries marked unused in the `$MFT $BITMAP`.
    pub fn from_raw_mft(raw_mft: &RawMft<'_>) -> crate::UsnResult<Self> {
        let mut entries = FxHashMap::with_capacity_and_hasher(
            raw_mft.record_count() as usize,
            Default::default(),
        );
        for r in raw_mft.try_iter()? {
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
    #[doc(hidden)]
    pub fn insert(&mut self, fid: u64, parent: u64, name: &[u16]) {
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

    pub(super) fn resolve_with_optional_drive(
        &self,
        fid: Fid,
        drive: Option<char>,
    ) -> Option<PathBuf> {
        // Maximum walk depth — far above the practical NTFS path-component
        // limit (~64 segments) and below any realistic cycle length.
        const MAX_STEPS: usize = 256;

        let mut chain: Vec<&[u16]> = Vec::with_capacity(32);
        let mut current = mask_fid_to_record_number(fid)?;
        let mut steps = 0usize;
        loop {
            if steps >= MAX_STEPS {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn resolve_four_deep_path() {
        // Layout:
        //   5 (root)  -> "Users" (10) -> "alice" (20) -> "docs" (30) -> "todo.txt" (40)
        let mut tree = InMemoryDirTree::default();
        tree.insert(10, 5, &utf16("Users"));
        tree.insert(20, 10, &utf16("alice"));
        tree.insert(30, 20, &utf16("docs"));
        tree.insert(40, 30, &utf16("todo.txt"));

        let p = tree.resolve(Fid::new(40)).expect("resolved");
        // Without drive prefix, components join with the platform separator.
        assert_eq!(
            p.to_string_lossy().replace('/', "\\"),
            "Users\\alice\\docs\\todo.txt"
        );

        let p = tree
            .resolve_with_drive_letter(Fid::new(40), 'c')
            .expect("with drive");
        assert_eq!(p.to_string_lossy(), "C:\\Users\\alice\\docs\\todo.txt");
    }

    #[test]
    fn cycle_detection() {
        let mut tree = InMemoryDirTree::default();
        // 10 -> 11 -> 10 (cycle)
        tree.insert(10, 11, &utf16("a"));
        tree.insert(11, 10, &utf16("b"));
        // The walker bounds at 256 steps; the chain visits 10, 11, 10, 11...
        // forever and returns None.
        assert!(tree.resolve(Fid::new(10)).is_none());
    }

    #[test]
    fn missing_fid_returns_none() {
        let tree = InMemoryDirTree::default();
        assert!(tree.resolve(Fid::new(0xDEADBEEF)).is_none());
    }

    #[test]
    fn fid_with_sequence_bits_is_masked() {
        let mut tree = InMemoryDirTree::default();
        tree.insert(10, 5, &utf16("hello"));
        // Lookup with the high 16 bits set (sequence number) must
        // still resolve to the same record.
        let fid = (0x0123u64 << 48) | 10;
        assert!(tree.resolve(Fid::new(fid)).is_some());
    }
}
