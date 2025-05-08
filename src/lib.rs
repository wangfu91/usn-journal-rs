//! # usn-journal-rs
//!
//! A Rust library for reading and enumerating the Windows NTFS USN (Update Sequence Number) change journal and Master File Table (MFT).
//!
//! This crate provides safe, ergonomic abstractions for accessing the USN change journal and MFT records on NTFS volumes. It enables applications to efficiently monitor, enumerate, and resolve file system changes and metadata on Windows systems.
//!
//! ## Features
//! - Enumerate USN journal records and MFT entries as Rust iterators
//! - Resolve file IDs to full paths
//! - Utilities for working with NTFS volumes and file metadata
//! - Safe wrappers over Windows API calls
//!
//! ## Example: Enumerate USN Journal
//! ```rust
//! use usn_journal_rs::{usn_journal, utils};
//!
//! let volume_handle = utils::get_volume_handle('C').unwrap();
//! let journal_data = usn_journal::query(volume_handle, true).unwrap();
//! let journal = usn_journal::UsnJournal::new(volume_handle, journal_data.UsnJournalID);
//! for entry in journal {
//!     println!("USN entry: {{:?}}", entry);
//! }
//! ```
//!
//! ## Platform
//! - Windows NTFS volumes only
//! - Requires appropriate privileges to access the USN journal
//!
//! ## License
//! MIT License. See [LICENSE](https://github.com/wangfu91/usn-journal-rs/blob/main/LICENSE).

pub mod mft;
pub mod path_resolver;
mod tests_utils;
pub mod usn_entry;
pub mod usn_journal;
pub mod utils;

pub type Usn = i64;

pub(crate) const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB

pub const DEFAULT_JOURNAL_MAX_SIZE: u64 = 32 * 1024 * 1024; // 32MB
pub const DEFAULT_JOURNAL_ALLOCATION_DELTA: u64 = 8 * 1024 * 1024; // 4MB

pub const USN_REASON_MASK_ALL: u32 = 0xFFFFFFFF;
