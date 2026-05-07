//! Path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve full file paths from file IDs using MFT or USN journal data.

mod entry;
pub mod in_memory_tree;
mod resolve;
mod resolver;
mod util;

pub use entry::PathResolvableEntry;
pub use in_memory_tree::InMemoryDirTree;
pub use resolver::PathResolver;

#[cfg(test)]
mod tests;
