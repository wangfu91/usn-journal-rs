//! `PathResolver` and its builder.

use lru::LruCache;
use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf};

use crate::{raw_mft::RawMft, volume::Volume};

use super::{
    InMemoryDirTree, PathResolvableEntry,
    resolve::{DirLruCache, resolve_path, resolve_path_with_cache},
};

/// Default number of directory paths cached by [`PathResolver::new`].
#[allow(clippy::useless_nonzero_new_unchecked)]
pub const DEFAULT_LRU_CACHE_CAPACITY: NonZeroUsize = unsafe {
    // SAFETY: `4096` is a non-zero constant.
    NonZeroUsize::new_unchecked(4096)
};

/// Resolves file paths from file IDs on an NTFS/ReFS volume.
///
/// Use [`PathResolver::new`] to configure and construct an instance:
///
/// ```no_run
/// use usn_journal_rs::{volume::Volume, path::PathResolver};
/// use std::num::NonZeroUsize;
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Default resolver — syscall resolution with an LRU directory cache:
/// let resolver = PathResolver::new(&volume);
///
/// // Tune the LRU directory cache for repeated lookups in the same directory:
/// let resolver = PathResolver::new(&volume)
///     .with_lru_cache(NonZeroUsize::new(8_192).unwrap());
/// ```
///
/// `PathResolver` is intentionally `!Sync` — it carries an internal
/// scratch buffer (and optional in-memory tree) accessed via interior
/// mutability to keep the public `resolve_path` signature ergonomic.
#[derive(Debug)]
pub struct PathResolver<'a> {
    /// Volume on which file IDs will be resolved.
    volume: &'a Volume,
    /// Optional cache of previously resolved directory paths.
    pub(super) dir_fid_path_cache: Option<DirLruCache>,
    /// Reusable heap buffer for `GetFileInformationByHandleEx` calls.
    buffer: RefCell<Vec<u8>>,
    /// Optional fully in-memory NTFS directory tree.
    pub(super) in_memory_tree: Option<InMemoryDirTree>,
}

impl<'a> PathResolver<'a> {
    /// Create a resolver with the given `volume` and the default LRU directory cache.
    ///
    /// Use [`Self::with_lru_cache`] to resize the cache,
    /// [`Self::without_lru_cache`] to force uncached syscall resolution, or
    /// [`Self::with_in_memory_tree`] to add the raw-`$MFT` directory tree.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        Self {
            volume,
            dir_fid_path_cache: Some(LruCache::new(DEFAULT_LRU_CACHE_CAPACITY)),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: None,
        }
    }

    /// Enable or resize the LRU directory path cache.
    #[must_use]
    pub fn with_lru_cache(mut self, capacity: NonZeroUsize) -> Self {
        self.dir_fid_path_cache = Some(LruCache::new(capacity));
        self
    }

    /// Disable the LRU directory path cache.
    ///
    /// Resolution still uses the in-memory directory tree when one is
    /// configured; otherwise each lookup falls back to direct syscalls.
    #[must_use]
    pub fn without_lru_cache(mut self) -> Self {
        self.dir_fid_path_cache = None;
        self
    }

    /// Add an in-memory raw-`$MFT` directory tree for O(1) full-scan path resolution.
    pub fn with_in_memory_tree(mut self, raw_mft: &RawMft<'_>) -> crate::UsnResult<Self> {
        self.in_memory_tree = Some(InMemoryDirTree::from_raw_mft(raw_mft)?);
        Ok(self)
    }

    /// Resolve `entry` to a full path, using the in-memory tree (if
    /// configured), then the LRU cache (if configured), falling back to
    /// `OpenFileById` syscalls.
    ///
    /// Standard 64-bit NTFS IDs can use all resolver strategies. Extended
    /// 128-bit IDs (for example ReFS `USN_RECORD_V3` entries) skip the
    /// in-memory raw-`$MFT` tree and are resolved via `OpenFileById`.
    #[must_use]
    pub fn resolve_path<E: PathResolvableEntry>(&mut self, entry: &E) -> Option<PathBuf> {
        if let Some(tree) = &self.in_memory_tree
            && let Some(p) =
                tree.resolve_with_optional_drive(entry.fid(), self.volume.drive_letter())
        {
            return Some(p);
        }
        if let Some(cache) = &mut self.dir_fid_path_cache {
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
