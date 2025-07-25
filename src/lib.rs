//! # usn-journal-rs
//!
//! A Rust library for manipulating the NTFS/ReFS USN change journal and enumerating the NTFS Master File Table (MFT).
//!
//! This crate provides safe, ergonomic abstractions for accessing the USN change journal and MFT records on NTFS volumes.
//! It enables applications to efficiently monitor, enumerate file system changes on Windows.
//!
//! ## Features
//! - Enumerate USN journal records or MFT entries as Rust iterators
//! - Resolve file IDs to full paths
//! - Safe wrappers over Windows API calls
//!
//! ## Example: Enumerate USN Journal
//! ```rust
//! use usn_journal_rs::{volume::Volume, journal::UsnJournal};
//!
//! let drive_letter = 'C';
//! let volume = Volume::from_drive_letter(drive_letter).unwrap();
//! let journal = UsnJournal::new(&volume);
//! for result in journal.iter().unwrap().take(10) {
//!     match result {
//!         Ok(entry) => println!("USN entry: {:?}", entry),
//!         Err(e) => eprintln!("Error reading entry: {}", e),
//!     }
//! }
//! ```
//!
//! # Example: Enumerating MFT Entries
//! ```rust
//! use usn_journal_rs::{volume::Volume, mft::Mft};
//!
//! let drive_letter = 'C';
//! let volume = Volume::from_drive_letter(drive_letter).unwrap();
//! let mft = Mft::new(&volume);
//! for result in mft.iter().take(10) {
//!     match result {
//!         Ok(entry) => println!("MFT entry: {:?}", entry),
//!         Err(e) => eprintln!("Error reading MFT entry: {}", e),
//!     }
//! }
//! ```
//!
//! ## Platform
//! - Windows NTFS/ReFS volumes
//! - Requires appropriate privileges to access the USN journal
//!
//! ## License
//! MIT License. See [LICENSE](https://github.com/wangfu91/usn-journal-rs/blob/main/LICENSE).

pub mod errors;
pub mod journal;
pub mod mft;
pub mod path;
mod privilege;

// Re-export commonly used types
pub use errors::UsnError;

/// A convenient type alias for Results with UsnError.
pub type UsnResult<T> = std::result::Result<T, UsnError>;

mod time;
pub mod volume;

// Utility functions for cargo tests
#[cfg(test)]
mod tests;

pub type Usn = i64;

pub(crate) const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB

pub const DEFAULT_JOURNAL_MAX_SIZE: u64 = 32 * 1024 * 1024; // 32MB
pub const DEFAULT_JOURNAL_ALLOCATION_DELTA: u64 = 8 * 1024 * 1024; // 4MB
pub const USN_REASON_MASK_ALL: u32 = 0xFFFFFFFF;
