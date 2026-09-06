//! `PathResolver` and its builder.

use lru::LruCache;
use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf};

use crate::volume::Volume;

use super::{
    PathResolvableEntry,
    resolve::{DirLruCache, resolve_path, resolve_path_with_cache},
};

#[allow(clippy::useless_nonzero_new_unchecked)]
const DEFAULT_DIRECTORY_CACHE_CAPACITY: NonZeroUsize = unsafe {
    // SAFETY: `4096` is a non-zero constant.
    NonZeroUsize::new_unchecked(4096)
};

/// Resolves current on-disk paths from file IDs on an NTFS/ReFS volume.
///
/// Use [`PathResolver::new`] to configure and construct an instance:
///
/// ```no_run
/// use usn_journal_rs::{volume::Volume, path::PathResolver};
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Default resolver — syscall resolution with a directory cache:
/// let resolver = PathResolver::new(&volume);
///
/// // Tune the directory cache capacity (plain integer, no NonZeroUsize):
/// let resolver = PathResolver::new(&volume).with_directory_cache(8_192);
///
/// // Disable the directory cache entirely (pass 0):
/// let resolver = PathResolver::new(&volume).with_directory_cache(0);
/// ```
///
/// [`resolve_path`](Self::resolve_path) takes `&self`: the directory cache and
/// scratch buffer are held behind interior mutability, so a single resolver can
/// be shared across an iteration loop without a `mut` binding. `PathResolver` is
/// intentionally `!Sync` because that interior state is not synchronized.
#[derive(Debug)]
pub struct PathResolver<'a> {
    /// Volume on which file IDs will be resolved.
    volume: &'a Volume,
    /// Optional cache of previously resolved directory paths.
    pub(super) dir_fid_path_cache: RefCell<Option<DirLruCache>>,
    /// Reusable heap buffer for `GetFileInformationByHandleEx` calls.
    buffer: RefCell<Vec<u8>>,
}

impl<'a> PathResolver<'a> {
    /// Create a resolver with the given `volume` and the default directory cache.
    ///
    /// Use [`Self::with_directory_cache`] to resize or disable the cache.
    ///
    /// This resolver is intended for live/current path resolution against the
    /// mounted volume.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        Self {
            volume,
            dir_fid_path_cache: RefCell::new(Some(LruCache::new(DEFAULT_DIRECTORY_CACHE_CAPACITY))),
            buffer: RefCell::new(Vec::new()),
        }
    }

    /// Set the directory path cache capacity.
    ///
    /// Pass a positive `capacity` to enable (or resize) the cache; pass `0`
    /// to disable it entirely.  When the cache is disabled, each directory
    /// lookup falls back to direct `OpenFileById` syscalls.
    ///
    /// The default resolver created by [`Self::new`] already has a built-in
    /// cache capacity.
    #[must_use]
    pub fn with_directory_cache(mut self, capacity: usize) -> Self {
        self.dir_fid_path_cache = RefCell::new(NonZeroUsize::new(capacity).map(LruCache::new));
        self
    }

    /// Resolve `entry` to its current on-disk path, using the directory cache
    /// when configured and falling back to `OpenFileById` syscalls.
    ///
    /// Takes `&self`: the cache and scratch buffer are updated through interior
    /// mutability, so the same resolver can be reused across an iterator without
    /// a `mut` binding.
    ///
    /// Standard 64-bit NTFS IDs and extended 128-bit IDs (for example ReFS
    /// `USN_RECORD_V3` entries) are both resolved via the live volume.
    #[must_use]
    pub fn resolve_path<E: PathResolvableEntry + ?Sized>(&self, entry: &E) -> Option<PathBuf> {
        let mut cache_guard = self.dir_fid_path_cache.borrow_mut();
        if let Some(cache) = cache_guard.as_mut() {
            resolve_path_with_cache(
                self.volume,
                entry.fid(),
                entry.parent_fid(),
                entry.file_name(),
                entry.is_dir(),
                cache,
                &self.buffer,
            )
        } else {
            drop(cache_guard);
            resolve_path(
                self.volume,
                entry.fid(),
                entry.parent_fid(),
                entry.file_name(),
                &self.buffer,
            )
        }
    }
}

impl Volume {
    /// Create a live [`PathResolver`] for this volume.
    ///
    /// Convenience for [`PathResolver::new`] (includes the default directory
    /// cache).
    #[must_use]
    pub fn path_resolver(&self) -> PathResolver<'_> {
        PathResolver::new(self)
    }
}
