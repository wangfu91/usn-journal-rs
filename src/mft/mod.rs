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
pub use options::{MftIterOptions, MftIterOptionsBuilder, UsnRecordVersion};

/// Compatibility name for [`MftIterOptions`].
/// The 0.4.1 public fields and struct literals still require migration to builders.
#[deprecated(since = "0.5.0", note = "Use MftIterOptions and its builder")]
pub type EnumOptions = MftIterOptions;

#[cfg(test)]
mod tests;
