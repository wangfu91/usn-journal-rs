//! Internal NTFS on-disk structures and parsers used by the raw `$MFT` reader.

pub(crate) mod attribute;
pub(crate) mod boot;
pub(crate) mod data_run;
pub(crate) mod extent;
pub(crate) mod fixup;
pub(crate) mod record;
