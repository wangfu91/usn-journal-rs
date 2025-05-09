//! Path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve full file paths from file IDs using MFT or USN journal data.

use crate::{
    mft::{Mft, MftEntry},
    usn_journal::{UsnEntry, UsnJournal},
    utils,
};
use lru::LruCache;
use std::{ffi::OsString, num::NonZeroUsize, path::PathBuf};
use windows::Win32::Foundation::HANDLE;

const LRU_CACHE_CAPACITY: usize = 4 * 1024; // 4KB

/// Resolves file paths from file IDs on an NTFS/ReFS volume, using an LRU cache for efficiency.
#[derive(Debug)]
struct PathResolver {
    volume_handle: HANDLE,                  // Handle to the NTFS/ReFS volume
    drive_letter: Option<char>,             // Optional drive letter (e.g., 'C')
    fid_path_cache: LruCache<u64, PathBuf>, // LRU cache for file ID to path mapping
}

/// Path resolver for MFT-based lookups.
pub struct MftPathResolver {
    path_resolver: PathResolver,
}

impl MftPathResolver {
    /// Create a new `MftPathResolver` from an MFT reference.
    pub fn new(mft: &Mft) -> Self {
        let path_resolver = PathResolver::new(mft.volume_handle, mft.drive_letter);
        MftPathResolver { path_resolver }
    }

    /// Resolve the full path for a given MFT entry.
    ///
    /// # Arguments
    /// * `mft_entry` - Reference to the MFT entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    pub fn resolve_path(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.path_resolver.resolve_path_from_mft(mft_entry)
    }
}

/// Path resolver for USN journal-based lookups.
pub struct UsnJournalPathResolver {
    path_resolver: PathResolver,
}

impl UsnJournalPathResolver {
    /// Create a new `UsnJournalPathResolver` from a USN journal reference.
    pub fn new(journal: &UsnJournal) -> Self {
        let path_resolver = PathResolver::new(journal.volume_handle, journal.drive_letter);
        UsnJournalPathResolver { path_resolver }
    }

    /// Resolve the full path for a given USN entry.
    ///
    /// # Arguments
    /// * `usn_entry` - Reference to the USN entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    pub fn resolve_path(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.path_resolver.resolve_path_from_usn(usn_entry)
    }
}

impl PathResolver {
    /// Create a new `PathResolver` for a given NTFS/ReFs volume and drive letter.
    ///
    /// # Arguments
    /// * `volume_handle` - Handle to the NTFS/ReFS volume.
    /// * `drive_letter` - Optional drive letter (e.g., 'C').
    fn new(volume_handle: HANDLE, drive_letter: Option<char>) -> Self {
        let fid_path_cache = LruCache::new(NonZeroUsize::new(LRU_CACHE_CAPACITY).unwrap());
        PathResolver {
            volume_handle,
            drive_letter,
            fid_path_cache,
        }
    }

    /// Resolve the full path for a given MFT entry.
    ///
    /// Uses the LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    fn resolve_path_from_mft(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.resolve_path(mft_entry.fid, mft_entry.parent_fid, &mft_entry.file_name)
    }

    /// Resolve the full path for a given USN entry.
    ///
    /// Uses the LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    fn resolve_path_from_usn(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.resolve_path(usn_entry.fid, usn_entry.parent_fid, &usn_entry.file_name)
    }

    /// Internal: Resolve the full path from file ID, parent file ID, and file name.
    ///
    /// # Arguments
    /// * `fid` - File ID of the target file.
    /// * `parent_fid` - File ID of the parent directory.
    /// * `file_name` - File or directory name.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    fn resolve_path(&mut self, fid: u64, parent_fid: u64, file_name: &OsString) -> Option<PathBuf> {
        if let Some(path) = self.fid_path_cache.get(&fid) {
            return Some(path.clone());
        }

        if let Some(parent_path) = self.fid_path_cache.get(&parent_fid) {
            let path = parent_path.join(file_name);
            self.fid_path_cache.put(fid, path.clone());
            return Some(path);
        }

        // If not in cache, try to get parent path from file system
        if let Ok(parent_path) =
            utils::file_id_to_path(self.volume_handle, self.drive_letter, parent_fid)
        {
            let path = parent_path.join(file_name);
            self.fid_path_cache.put(parent_fid, parent_path);
            self.fid_path_cache.put(fid, path.clone());
            return Some(path);
        }

        None
    }
}
