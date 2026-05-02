//! Master File Table (MFT) enumeration support.
//!
//! This module provides the high-level `Mft` wrapper plus the owned entry,
//! iterator, and option types used to enumerate `FSCTL_ENUM_USN_DATA`
//! records from an NTFS volume.

mod entry;
mod iter;
#[allow(clippy::module_inception)]
mod mft;
mod options;

pub use entry::MftEntry;
pub use iter::MftIter;
pub use mft::Mft;
pub use options::{MftIterOptions, MftIterOptionsBuilder};

#[cfg(test)]
mod tests;