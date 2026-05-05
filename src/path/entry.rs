//! Traits for entries that can be resolved into filesystem paths.

use std::ffi::OsString;

use crate::{Fid, journal::UsnEntry, mft::MftEntry};

/// Trait for entries that can be resolved to a file path.
pub trait PathResolvableEntry {
    /// Return the entry's file identifier.
    fn fid(&self) -> Fid;
    /// Return the parent directory's file identifier.
    fn parent_fid(&self) -> Fid;
    /// Return the entry's leaf file name.
    fn file_name(&self) -> &OsString;
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
    fn file_name(&self) -> &OsString {
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
    fn file_name(&self) -> &OsString {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_dir()
    }
}
