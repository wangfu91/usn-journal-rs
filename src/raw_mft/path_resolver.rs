//! Snapshot path resolver for entries produced by a raw `$MFT` scan.

use std::{cell::RefCell, path::PathBuf};

use crate::{
    UsnResult,
    path::{InMemoryDirTree, resolve::resolve_path as resolve_live_path},
    volume::Volume,
};

use super::{RawMft, RawMftEntry};

/// Path resolver specialized for entries produced by [`RawMft`].
///
/// By default this resolver is snapshot-local: it walks a pre-built in-memory
/// directory tree derived from the same raw-`$MFT` scan and does not issue any
/// live `OpenFileById` lookups. This keeps raw-`$MFT` path reconstruction tied
/// to one point-in-time snapshot and avoids mixing live filesystem state into
/// the default result.
///
/// If you explicitly want best-effort live fallback for paths that are missing
/// from the snapshot tree, call [`Self::with_live_fallback`].
#[derive(Debug)]
pub struct RawMftPathResolver<'a> {
    /// Volume from which the raw `$MFT` snapshot was built.
    volume: &'a Volume,
    /// Snapshot-local directory tree built from the raw `$MFT` scan.
    in_memory_tree: InMemoryDirTree,
    /// Whether live `OpenFileById` fallback is allowed when the snapshot tree misses.
    live_fallback: bool,
    /// Reusable heap buffer for optional live fallback lookups.
    buffer: RefCell<Vec<u8>>,
}

impl<'a> RawMftPathResolver<'a> {
    /// Build a snapshot-local resolver from a raw `$MFT` reader.
    pub(crate) fn new(raw_mft: &RawMft<'a>) -> UsnResult<Self> {
        Ok(Self {
            volume: raw_mft.volume(),
            in_memory_tree: InMemoryDirTree::try_from(raw_mft)?,
            live_fallback: false,
            buffer: RefCell::new(Vec::new()),
        })
    }

    /// Enable best-effort live fallback for entries missing from the snapshot tree.
    ///
    /// This is opt-in because live fallback mixes current filesystem state into
    /// results derived from a point-in-time raw-`$MFT` snapshot.
    #[must_use]
    pub fn with_live_fallback(mut self) -> Self {
        self.live_fallback = true;
        self
    }

    /// Resolve `entry` to a path using the raw-`$MFT` snapshot tree.
    ///
    /// When [`Self::with_live_fallback`] has been enabled, unresolved snapshot
    /// entries fall back to a live `OpenFileById` lookup against the mounted volume.
    #[must_use]
    pub fn resolve_path(&self, entry: &RawMftEntry) -> Option<PathBuf> {
        self.in_memory_tree
            .resolve_with_optional_drive(entry.file_reference, self.volume.drive_letter())
            .or_else(|| {
                if self.live_fallback {
                    resolve_live_path(
                        self.volume,
                        entry.file_reference,
                        entry.parent_reference,
                        entry.file_name.as_os_str(),
                        &self.buffer,
                    )
                } else {
                    None
                }
            })
    }
}
