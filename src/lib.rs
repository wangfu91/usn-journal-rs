//! # usn-journal-rs
//!
//! A Rust library for manipulating the NTFS/ReFS USN change journal and enumerating the NTFS Master File Table (MFT).
//!
//! This crate provides safe, ergonomic abstractions for accessing the USN
//! change journal and MFT records on NTFS/ReFS volumes.
//! It enables applications to efficiently monitor, enumerate file system changes on Windows.
//!
//! ## Features
//! - Enumerate USN journal records or MFT entries as Rust iterators
//! - Parse both `USN_RECORD_V2` and `USN_RECORD_V3` records
//! - Resolve file IDs to full paths
//! - Safe wrappers over Windows API calls
//!
//! ## Example: Enumerate USN Journal
//! ```no_run
//! use usn_journal_rs::{volume::Volume, journal::UsnJournal};
//!
//! let drive_letter = 'C';
//! let volume = Volume::from_drive_letter(drive_letter).unwrap();
//! let journal = UsnJournal::new(&volume);
//! for result in journal.try_iter().unwrap().take(10) {
//!     match result {
//!         Ok(entry) => println!("USN entry: {entry:?}"),
//!         Err(e) => eprintln!("Error reading entry: {e}"),
//!     }
//! }
//! ```
//!
//! # Example: Enumerating MFT Entries
//! ```no_run
//! use usn_journal_rs::{volume::Volume, mft::Mft};
//!
//! let drive_letter = 'C';
//! let volume = Volume::from_drive_letter(drive_letter).unwrap();
//! let mft = Mft::new(&volume);
//! for result in mft.try_iter().unwrap().take(10) {
//!     match result {
//!         Ok(entry) => println!("MFT entry: {entry:?}"),
//!         Err(e) => eprintln!("Error reading MFT entry: {e}"),
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
mod file_attributes;
pub mod journal;
pub mod mft;
pub mod path;
mod privilege;
pub mod raw_mft;
pub mod types;
mod unaligned;
mod usn_record;

// Re-export commonly used types
pub use errors::UsnError;
pub use types::{Fid, FileAttributes, Usn, UsnReason, UsnSourceInfo};

/// A convenient type alias for Results with UsnError.
pub type UsnResult<T> = std::result::Result<T, UsnError>;

/// Common imports for applications using the crate.
pub mod prelude {
    pub use crate::{
        Fid, FileAttributes, Filetime, Usn, UsnError, UsnReason, UsnResult, UsnSourceInfo,
        journal::UsnJournal, mft::Mft, path::PathResolver, raw_mft::RawMft, volume::Volume,
    };
}

/// Windows FILETIME wrapper used throughout the crate.
#[doc(inline)]
pub use time::Filetime;

mod time;
pub mod volume;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::prelude;

    #[test]
    fn prelude_exports_common_types() {
        fn accepts<T>() {}

        accepts::<prelude::Volume>();
        accepts::<prelude::UsnJournal>();
        accepts::<prelude::Mft>();
        accepts::<prelude::RawMft<'_>>();
        accepts::<prelude::PathResolver<'_>>();
        accepts::<prelude::UsnError>();
        accepts::<prelude::Usn>();
        accepts::<prelude::Fid>();
        accepts::<prelude::Filetime>();
        accepts::<prelude::UsnReason>();
        accepts::<prelude::FileAttributes>();
        accepts::<prelude::UsnSourceInfo>();

        let result: prelude::UsnResult<()> = Ok(());
        assert!(result.is_ok());
    }
}
