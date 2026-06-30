//! Traits for entries that can be resolved into filesystem paths.

use std::ffi::OsStr;

use crate::{Fid, journal::UsnEntry, mft::MftEntry};

pub(crate) mod sealed {
    use crate::{journal::UsnEntry, mft::MftEntry};

    pub trait Sealed {}

    impl Sealed for MftEntry {}
    impl Sealed for UsnEntry {}
}

/// Trait for live/current entries that can be resolved by [`crate::path::PathResolver`].
///
/// This trait is sealed to crate-defined entry types so the live path resolver
/// stays aligned with the semantics of the underlying enumeration APIs.
pub trait PathResolvableEntry: sealed::Sealed {
    /// Return the entry's file identifier.
    fn fid(&self) -> Fid;
    /// Return the parent directory's file identifier.
    fn parent_fid(&self) -> Fid;
    /// Return the entry's leaf file name.
    fn file_name(&self) -> &OsStr;
    /// Return whether the entry represents a directory.
    fn is_dir(&self) -> bool;
}

impl PathResolvableEntry for MftEntry {
    fn fid(&self) -> Fid {
        self.fid
    }
    fn parent_fid(&self) -> Fid {
        self.parent_fid
    }
    fn file_name(&self) -> &OsStr {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_dir()
    }
}

impl PathResolvableEntry for UsnEntry {
    fn fid(&self) -> Fid {
        self.fid
    }
    fn parent_fid(&self) -> Fid {
        self.parent_fid
    }
    fn file_name(&self) -> &OsStr {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_dir()
    }
}
