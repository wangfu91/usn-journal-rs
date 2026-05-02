//! `PathResolver` and its builder.

use lru::LruCache;
use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf};

use crate::{raw_mft::RawMft, volume::Volume};

use super::{
    InMemoryDirTree, PathResolvableEntry,
    resolve::{DirLruCache, resolve_path, resolve_path_with_cache},
};

/// Resolves file paths from file IDs on an NTFS/ReFS volume.
///
/// Use [`PathResolver::builder`] to configure and construct an instance:
///
/// ```no_run
/// use usn_journal_rs::{volume::Volume, path::PathResolver};
/// use std::num::NonZeroUsize;
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Default resolver — uncached, pure syscall resolution:
/// let resolver = PathResolver::builder(&volume).build();
///
/// // With an LRU directory cache for repeated lookups in the same directory:
/// let resolver = PathResolver::builder(&volume)
///     .with_lru_cache(NonZeroUsize::new(4_096).unwrap())
///     .build();
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
    /// Create a [`PathResolverBuilder`] for the given volume.
    ///
    /// The builder defaults to uncached syscall resolution. Use
    /// [`PathResolverBuilder::with_lru_cache`] to add a directory path cache, or
    /// [`PathResolverBuilder::build_with_in_memory_tree`] for the fastest NTFS
    /// full-scan strategy.
    #[must_use]
    pub fn builder(volume: &'a Volume) -> PathResolverBuilder<'a> {
        PathResolverBuilder::new(volume)
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
/// // Uncached resolver (the default):
/// let mut resolver = PathResolver::builder(&volume).build();
///
/// // With an LRU cache:
/// let mut resolver = PathResolver::builder(&volume)
///     .with_lru_cache(NonZeroUsize::new(4096).unwrap())
///     .build();
///
/// // With in-memory tree (NTFS only):
/// let raw_mft = RawMft::new(&volume).unwrap();
/// let mut resolver = PathResolver::builder(&volume)
///     .build_with_in_memory_tree(&raw_mft)
///     .unwrap();
/// ```
#[derive(Debug)]
pub struct PathResolverBuilder<'a> {
    volume: &'a Volume,
    lru_cache_capacity: Option<NonZeroUsize>,
}

impl<'a> PathResolverBuilder<'a> {
    pub(super) fn new(volume: &'a Volume) -> Self {
        PathResolverBuilder {
            volume,
            lru_cache_capacity: None,
        }
    }

    /// Enable an LRU directory path cache with the given capacity.
    ///
    /// By default the builder produces an uncached resolver. Calling this
    /// method enables a cache that avoids repeated `OpenFileById` round-trips
    /// for files in the same directory.
    ///
    /// # Example
    /// ```no_run
    /// use usn_journal_rs::{volume::Volume, path::PathResolver};
    /// use std::num::NonZeroUsize;
    ///
    /// let volume = Volume::from_drive_letter('C').unwrap();
    /// let resolver = PathResolver::builder(&volume)
    ///     .with_lru_cache(NonZeroUsize::new(4096).unwrap())
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_lru_cache(mut self, capacity: NonZeroUsize) -> Self {
        self.lru_cache_capacity = Some(capacity);
        self
    }

    /// Build a [`PathResolver`] without an in-memory directory tree.
    #[must_use]
    pub fn build(self) -> PathResolver<'a> {
        PathResolver {
            volume: self.volume,
            dir_fid_path_cache: self.lru_cache_capacity.map(LruCache::new),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: None,
        }
    }

    /// Build a [`PathResolver`] backed by an in-memory directory tree built
    /// from the given raw `$MFT`.
    ///
    /// Path resolution checks the tree first; on a miss it falls back to
    /// `OpenFileById` syscalls (and caches the result when the LRU cache is
    /// enabled).
    ///
    /// Returns an error on non-NTFS volumes (e.g. ReFS) or if the MFT
    /// iteration fails.
    ///
    /// # Example
    /// ```no_run
    /// use usn_journal_rs::{volume::Volume, raw_mft::RawMft, path::PathResolver};
    ///
    /// let volume = Volume::from_drive_letter('C').unwrap();
    /// let raw_mft = RawMft::new(&volume).unwrap();
    /// let mut resolver = PathResolver::builder(&volume)
    ///     .build_with_in_memory_tree(&raw_mft)
    ///     .unwrap();
    /// for entry in raw_mft.try_iter().unwrap().flatten().take(100) {
    ///     if let Some(path) = resolver.resolve_path(&entry) {
    ///         println!("{}", path.display());
    ///     }
    /// }
    /// ```
    pub fn build_with_in_memory_tree(
        self,
        raw_mft: &RawMft<'_>,
    ) -> crate::UsnResult<PathResolver<'a>> {
        let tree = InMemoryDirTree::from_raw_mft(raw_mft)?;
        Ok(PathResolver {
            volume: self.volume,
            dir_fid_path_cache: self.lru_cache_capacity.map(LruCache::new),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: Some(tree),
        })
    }
}