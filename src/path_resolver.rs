use std::{ffi::OsString, num::NonZeroUsize, path::PathBuf};

use lru::LruCache;
use windows::Win32::Foundation::HANDLE;

use crate::{mft::MftEntry, usn_entry::UsnEntry, utils};

const CACHE_CAPACITY: usize = 4 * 1024; // 4KB

/// A struct to resolve file paths from file IDs on an NTFS volume.
pub struct PathResolver {
    volume_handle: HANDLE,
    drive_letter: char,
    fid_path_cache: LruCache<u64, PathBuf>,
}

impl PathResolver {
    /// Create a new `PathResolver` for a given NTFS volume and drive letter.
    ///
    /// # Arguments
    /// * `volume_handle` - Handle to the NTFS volume.
    /// * `drive_letter` - The drive letter (e.g., 'C').
    pub fn new(volume_handle: HANDLE, drive_letter: char) -> Self {
        let fid_path_cache = LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).unwrap());
        PathResolver {
            volume_handle,
            drive_letter,
            fid_path_cache,
        }
    }

    /// Resolve the full path for a given MFT entry.
    ///
    /// Uses an LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    pub fn resolve_path_from_mft(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.resolve_path(mft_entry.fid, mft_entry.parent_fid, &mft_entry.file_name)
    }

    /// Resolve the full path for a given USN entry.
    ///
    /// Uses an LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    pub fn resolve_path_from_usn(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.resolve_path(usn_entry.fid, usn_entry.parent_fid, &usn_entry.file_name)
    }

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
