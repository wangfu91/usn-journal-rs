//! Traits for entries that can be resolved into filesystem paths.

use std::ffi::OsString;

use crate::{Fid, journal::UsnEntry, mft::MftEntry};

/// Trait for entries that can be resolved to a file path.
pub trait PathResolvableEntry {
    fn fid(&self) -> Fid;
    fn parent_fid(&self) -> Fid;
    fn file_name(&self) -> &OsString;
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
