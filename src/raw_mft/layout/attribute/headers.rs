//! NTFS attribute on-disk layouts, enums, and flags.
//!
//! This submodule contains the packed structs that mirror bytes stored in
//! FILE record attribute streams plus the small enums/constants used to
//! classify them. Parsing logic lives in `view.rs`, while record traversal
//! lives in `iter.rs`.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Attribute type identifiers used by NTFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum NtfsAttributeType {
    /// `$STANDARD_INFORMATION`
    StandardInformation = 0x10,
    /// `$ATTRIBUTE_LIST`
    AttributeList = 0x20,
    /// `$FILE_NAME`
    FileName = 0x30,
    /// `$DATA`
    Data = 0x80,
    /// `$REPARSE_POINT`
    ReparsePoint = 0xC0,
    /// `$BITMAP`
    Bitmap = 0xB0,
    /// End-of-attributes marker.
    End = 0xFFFF_FFFF,
}

/// Raw on-disk attribute header common to both resident and non-resident
/// attributes.
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct NtfsAttributeHeader {
    /// Attribute type code.
    pub type_id: u32,
    /// Total length of this attribute record in bytes.
    pub length: u32,
    /// Non-zero when the attribute is non-resident.
    pub is_non_resident: u8,
    /// Length of the attribute name in UTF-16 code units.
    pub name_length: u8,
    /// Offset of the optional attribute name within the record.
    pub name_offset: u16,
    /// On-disk attribute flags.
    pub flags: u16,
    /// Attribute instance identifier.
    pub id: u16,
}

/// Resident attribute header layout.
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct NtfsResidentAttributeHeader {
    /// Common attribute header.
    pub attribute_header: NtfsAttributeHeader,
    /// Byte length of the resident value payload.
    pub value_length: u32,
    /// Byte offset of the resident value payload.
    pub value_offset: u16,
    /// Whether the attribute is indexed.
    pub indexed_flag: u8,
    /// Padding byte.
    pub _pad: u8,
}

/// Non-resident attribute header layout.
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct NtfsNonResidentAttributeHeader {
    /// Common attribute header.
    pub attribute_header: NtfsAttributeHeader,
    /// Lowest VCN covered by this attribute instance.
    pub lowest_vcn: i64,
    /// Highest VCN covered by this attribute instance.
    pub highest_vcn: i64,
    /// Byte offset of the encoded data runs.
    pub data_runs_offset: u16,
    /// Compression unit exponent.
    pub compression_unit_exponent: u8,
    /// Reserved bytes.
    pub _reserved: [u8; 5],
    /// Allocated size of the stream in bytes.
    pub allocated_size: u64,
    /// Logical data size in bytes.
    pub data_size: u64,
    /// Initialized data size in bytes.
    pub initialized_size: u64,
}

/// Standard information attribute (`$STANDARD_INFORMATION`, 0x10).
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct NtfsStandardInformation {
    /// FILETIME creation timestamp.
    pub creation_time: u64,
    /// FILETIME last-modified timestamp.
    pub modification_time: u64,
    /// FILETIME MFT-record-modified timestamp.
    pub mft_record_modification_time: u64,
    /// FILETIME last-access timestamp.
    pub access_time: u64,
    /// Raw file-attribute bitmask.
    pub file_attributes: u32,
}

/// File-name attribute namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FileNameNamespace {
    /// POSIX name entry.
    Posix = 0,
    /// Win32 long-name entry.
    Win32 = 1,
    /// DOS 8.3 short-name entry.
    Dos = 2,
    /// Combined Win32 + DOS entry.
    Win32AndDos = 3,
}

impl FileNameNamespace {
    /// Convert the on-disk namespace byte to the closest enum variant.
    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Posix,
            1 => Self::Win32,
            2 => Self::Dos,
            _ => Self::Win32AndDos,
        }
    }

    /// Ordering score for choosing the best file-name attribute when a
    /// record carries multiple. Higher is preferred.
    ///
    /// `Win32AndDos` > `Win32` > `Posix` > `Dos`
    #[inline]
    pub(crate) fn score(self) -> i32 {
        match self {
            FileNameNamespace::Win32AndDos => 4,
            FileNameNamespace::Win32 => 3,
            FileNameNamespace::Posix => 2,
            FileNameNamespace::Dos => 1,
        }
    }
}

/// On-disk header of a single entry in an `$ATTRIBUTE_LIST` attribute.
///
/// Each entry describes one attribute of the file and tells the kernel
/// which FILE record on disk holds that attribute. When the entry's
/// `file_reference` refers to a different record than the base record,
/// the attribute lives in an extension record.
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct AttributeListEntryHeader {
    /// Attribute type code (same values as [`NtfsAttributeType`]).
    pub type_id: u32,
    /// Total byte length of this entry (including name, rounded to 8 bytes).
    pub record_length: u16,
    /// Length of the optional attribute name in UTF-16 units.
    pub attribute_name_length: u8,
    /// Byte offset to the name from the start of this entry.
    pub attribute_name_offset: u8,
    /// Lowest VCN covered by this attribute instance (0 for resident attrs).
    pub lowest_vcn: i64,
    /// MFT file reference: 48-bit record number + 16-bit sequence number.
    pub file_reference: u64,
    /// Attribute instance identifier.
    pub attribute_id: u16,
}

/// File-name attribute fixed header.
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct NtfsFileNameHeader {
    /// Parent directory file reference.
    pub parent_directory_reference: u64,
    /// FILETIME creation timestamp.
    pub creation_time: u64,
    /// FILETIME last-modified timestamp.
    pub modification_time: u64,
    /// FILETIME MFT-record-modified timestamp.
    pub mft_record_modification_time: u64,
    /// FILETIME last-access timestamp.
    pub access_time: u64,
    /// Allocated stream size in bytes.
    pub allocated_size: u64,
    /// Logical stream size in bytes.
    pub real_size: u64,
    /// Raw file-attribute bitmask.
    pub file_attributes: u32,
    /// Reparse tag when `file_attributes` includes `REPARSE_POINT`.
    pub reparse_point_tag: u32,
    /// File-name length in UTF-16 code units.
    pub name_length: u8,
    /// File-name namespace selector.
    pub namespace: u8,
}

/// File-attribute flag bits (FILE_NAME / STANDARD_INFORMATION).
pub mod file_attr_flags {
    /// Sparse-file attribute bit.
    pub const SPARSE_FILE: u32 = 0x0200;
    /// Reparse-point attribute bit.
    pub const REPARSE_POINT: u32 = 0x0400;
    /// Compressed-file attribute bit.
    pub const COMPRESSED: u32 = 0x0800;
    /// Encrypted-file attribute bit.
    pub const ENCRYPTED: u32 = 0x4000;
}

