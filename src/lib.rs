#![deny(missing_docs)]

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
//! use usn_journal_rs::volume::Volume;
//!
//! # #[cfg(windows)] {
//! let volume = Volume::from_drive_letter('C').unwrap();
//! for result in volume.journal().try_iter().unwrap().take(10) {
//!     match result {
//!         Ok(entry) => println!("USN entry: {entry}"),
//!         Err(e) => eprintln!("Error reading entry: {e}"),
//!     }
//! }
//! # }
//! ```
//!
//! # Example: Enumerating MFT Entries
//! ```no_run
//! use usn_journal_rs::volume::Volume;
//!
//! # #[cfg(windows)] {
//! let volume = Volume::from_drive_letter('C').unwrap();
//! for result in volume.mft().try_iter().unwrap().take(10) {
//!     match result {
//!         Ok(entry) => println!("MFT entry: {entry}"),
//!         Err(e) => eprintln!("Error reading MFT entry: {e}"),
//!     }
//! }
//! # }
//! ```
//!
//! ## Platform
//! - Raw NTFS `$MFT` scanning is provided by the separate `ntfs-mft` crate.
//! - USN journal, FSCTL MFT enumeration, and live file-ID lookup are Windows-only.
//! - Raw devices are always opened read-only; appropriate OS permissions are required.
//!
//! ## License
//! MIT License. See [LICENSE](https://github.com/wangfu91/usn-journal-rs/blob/main/LICENSE).

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("usn-journal-rs supports only Windows and Linux targets");

mod display;
pub mod errors;
mod file_attributes;
#[cfg(windows)]
pub mod journal;
#[cfg(windows)]
pub mod mft;
pub mod path;
#[cfg(windows)]
pub mod privilege;
pub mod types;
#[cfg(windows)]
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
        volume::Volume,
    };
    #[cfg(windows)]
    pub use crate::{
        journal::{JournalIterOptions, UsnEntry, UsnJournal},
        mft::{Mft, MftEntry, MftIterOptions, UsnRecordVersion}, path::PathResolver,
    };
}

/// Windows FILETIME wrapper used throughout the crate.
#[doc(inline)]
pub use time::Filetime;

mod time;
pub mod volume;

#[cfg(all(test, windows))]
mod test_support;

#[cfg(all(test, windows))]
mod tests {
    use super::prelude;

    #[test]
    fn prelude_exports_common_types() {
        fn accepts<T>() {}

        accepts::<prelude::Volume>();
        accepts::<prelude::UsnJournal>();
        accepts::<prelude::Mft>();
        accepts::<prelude::PathResolver<'_>>();
        accepts::<prelude::UsnError>();
        accepts::<prelude::Usn>();
        accepts::<prelude::Fid>();
        accepts::<prelude::Filetime>();
        accepts::<prelude::UsnReason>();
        accepts::<prelude::FileAttributes>();
        accepts::<prelude::UsnSourceInfo>();
        accepts::<prelude::UsnEntry>();
        accepts::<prelude::MftEntry>();
        accepts::<prelude::JournalIterOptions>();
        accepts::<prelude::MftIterOptions>();
        accepts::<prelude::UsnRecordVersion>();

        let result: prelude::UsnResult<()> = Ok(());
        assert!(result.is_ok());
    }
}
