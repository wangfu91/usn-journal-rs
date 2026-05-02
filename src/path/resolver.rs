//! `PathResolver` and its builder.

use lru::LruCache;
use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf};

use crate::{raw_mft::RawMft, volume::Volume};

use super::{
    InMemoryDirTree, PathResolvableEntry,
    resolve::{DirLruCache, resolve_path, resolve_path_with_cache},
};

const DEFAULT_LRU_CACHE_CAPACITY: usize = 4096;

fn default_lru_cache_capacity() -> NonZeroUsize {
    NonZeroUsize::new(DEFAULT_LRU_CACHE_CAPACITY).expect("default cache capacity is non-zero")
}

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
    pub(super) volume: &'a Volume,
    pub(super) dir_fid_path_cache: Option<DirLruCache>,
    /// Reusable heap buffer for `GetFileInformationByHandleEx` calls.
    pub(super) buffer: RefCell<Vec<u8>>,
    pub(super) in_memory_tree: Option<InMemoryDirTree>,
}

impl<'a> PathResolver<'a> {
    /// Create a resolver with the default LRU directory cache enabled.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        Self {
            volume,
            dir_fid_path_cache: Some(LruCache::new(default_lru_cache_capacity())),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: None,
        }
    }

    /// Create a [`PathResolverBuilder`] for the given volume.
    ///
    /// The builder defaults to the same cached strategy as [`PathResolver::new`].
    /// Use [`PathResolverBuilder::without_lru_cache`] for fully uncached syscall
    /// resolution, or [`PathResolverBuilder::with_in_memory_tree`] for the
    /// fastest NTFS full-scan strategy.
    #[must_use]
    pub fn builder(volume: &'a Volume) -> PathResolverBuilder<'a> {
        PathResolverBuilder::new(volume)
    }

    /// Enable or resize the LRU directory path cache.
    #[must_use]
    pub fn with_lru_cache(mut self, capacity: NonZeroUsize) -> Self {
        self.dir_fid_path_cache = Some(LruCache::new(capacity));
        self
    }

    /// Disable the LRU directory path cache.
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

/// Builder for [`PathResolver`].
///
/// Obtain one via [`PathResolver::builder`].
///
/// # Example
/// ```no_run
/// use usn_journal_rs::{volume::Volume, raw_mft::RawMft, path::PathResolver};
/// use std::num::NonZeroUsize;
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Cached resolver (the default):
/// let mut resolver = PathResolver::builder(&volume).build();
///
/// // With a tuned LRU cache:
/// let mut resolver = PathResolver::builder(&volume)
///     .with_lru_cache(NonZeroUsize::new(4096).unwrap())
///     .build();
///
/// // With in-memory tree (NTFS only):
/// let raw_mft = RawMft::new(&volume).unwrap();
/// let mut resolver = PathResolver::builder(&volume)
///     .with_in_memory_tree(&raw_mft)
///     .unwrap()
///     .build();
/// ```
#[derive(Debug)]
pub struct PathResolverBuilder<'a> {
    volume: &'a Volume,
    lru_cache_capacity: Option<NonZeroUsize>,
    in_memory_tree: Option<InMemoryDirTree>,
}

impl<'a> PathResolverBuilder<'a> {
    pub(super) fn new(volume: &'a Volume) -> Self {
        PathResolverBuilder {
            volume,
            lru_cache_capacity: Some(default_lru_cache_capacity()),
            in_memory_tree: None,
        }
    }

    /// Enable an LRU directory path cache with the given capacity.
    ///
    /// The builder enables a default cache automatically. Calling this method
    /// changes the capacity.
    ///
    /// # Example
    /// ```no_run
    /// use usn_journal_rs::{volume::Volume, path::PathResolver};
    /// use std::num::NonZeroUsize;
    ///
    /// let volume = Volume::from_drive_letter('C').unwrap();
    /// let resolver = PathResolver::builder(&volume)
    ///     .with_lru_cache(NonZeroUsize::new(8192).unwrap())
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_lru_cache(mut self, capacity: NonZeroUsize) -> Self {
        self.lru_cache_capacity = Some(capacity);
        self
    }

    /// Disable the LRU directory path cache.
    #[must_use]
    pub fn without_lru_cache(mut self) -> Self {
        self.lru_cache_capacity = None;
        self
    }

    /// Build and attach an in-memory directory tree from a raw `$MFT` scan.
    pub fn with_in_memory_tree(mut self, raw_mft: &RawMft<'_>) -> crate::UsnResult<Self> {
        self.in_memory_tree = Some(InMemoryDirTree::from_raw_mft(raw_mft)?);
        Ok(self)
    }

    /// Build a [`PathResolver`] without an in-memory directory tree.
    #[must_use]
    pub fn build(self) -> PathResolver<'a> {
        PathResolver {
            volume: self.volume,
            dir_fid_path_cache: self.lru_cache_capacity.map(LruCache::new),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: self.in_memory_tree,
        }
    }
}
