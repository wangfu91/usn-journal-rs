//! NTFS attribute decoding.
//!
//! This module is split into small focused submodules:
//! - `headers`: on-disk NTFS attribute layouts, enums, and flags
//! - `view`: `NtfsAttribute` plus typed accessors for resident payloads
//! - `iter`: record-slice iterators over attributes and `$ATTRIBUTE_LIST` entries

mod headers;
mod iter;
mod view;

pub use headers::FileNameNamespace;
pub(crate) use headers::{
    AttributeListEntryHeader, NtfsAttributeHeader, NtfsAttributeType, NtfsFileNameHeader,
    NtfsNonResidentAttributeHeader, NtfsResidentAttributeHeader, NtfsStandardInformation,
    file_attr_flags,
};
pub(crate) use iter::{for_each_attr_list_entry, for_each_attribute};
pub(crate) use view::NtfsAttribute;
