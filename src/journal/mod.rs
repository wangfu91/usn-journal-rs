//! Provides access to the Windows NTFS/ReFS USN change journal.
//!
//! This module enables querying, creating, deleting, and iterating over the USN change journal on NTFS/ReFS volumes.
//! It provides safe Rust abstractions over the Windows API for monitoring file system changes efficiently.
//!

mod data;
mod defaults;
mod entry;
mod iter;
#[allow(clippy::module_inception)]
mod journal;
mod options;
mod reason;

pub use data::UsnJournalData;
pub use defaults::{
    DEFAULT_BUFFER_BYTES, DEFAULT_BUFFER_BYTES_NONZERO, DEFAULT_JOURNAL_ALLOCATION_DELTA,
    DEFAULT_JOURNAL_MAX_SIZE,
};
pub use entry::UsnEntry;
pub use iter::UsnJournalIter;
pub use journal::UsnJournal;
pub use options::{JournalIterOptions, JournalIterOptionsBuilder};

/// Compatibility name for [`JournalIterOptions`].
/// The 0.4.1 public fields and struct literals still require migration to builders.
#[deprecated(since = "0.5.0", note = "Use JournalIterOptions and its builder")]
pub type EnumOptions = JournalIterOptions;

#[cfg(test)]
mod tests;
