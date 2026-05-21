//! Live path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve current on-disk paths from file IDs
//! surfaced by the USN journal or `FSCTL_ENUM_USN_DATA`. For raw-`$MFT`
//! snapshot path reconstruction, use [`crate::raw_mft::RawMftPathResolver`].

mod entry;
mod in_memory_tree;
pub(crate) mod resolve;
mod resolver;

pub use entry::PathResolvableEntry;
pub(crate) use in_memory_tree::InMemoryDirTree;
pub use resolver::PathResolver;

#[cfg(test)]
mod tests;
