//! NTFS attribute decoding.
//!
//! Each FILE record contains a sequence of attribute records preceded by
//! either a resident or non-resident header. This module exposes a
//! lightweight view (`NtfsAttribute`) that borrows from a fixed-up record
//! buffer plus typed accessors for the attributes we care about.

use std::mem::size_of;

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
    /// record carries multiple.  Higher is preferred.
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
/// which FILE record on disk holds that attribute.  When the entry's
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

/// Minimum byte length of an `$ATTRIBUTE_LIST` entry (no attribute name).
const ATTR_LIST_ENTRY_MIN_SIZE: usize = size_of::<AttributeListEntryHeader>();

/// Call `f(type_id, file_reference)` for each valid entry in a raw
/// `$ATTRIBUTE_LIST` data slice.
///
/// Stops at the first malformed entry (zero-length or out of bounds).
pub(crate) fn for_each_attr_list_entry<F>(data: &[u8], mut f: F)
where
    F: FnMut(u32, u64),
{
    let mut offset = 0usize;
    while offset + ATTR_LIST_ENTRY_MIN_SIZE <= data.len() {
        let bytes = &data[offset..offset + ATTR_LIST_ENTRY_MIN_SIZE];
        let h = match AttributeListEntryHeader::read_from_bytes(bytes) {
            Ok(h) => h,
            Err(_) => break,
        };
        let len = h.record_length as usize;
        if len < ATTR_LIST_ENTRY_MIN_SIZE {
            break;
        }
        f(h.type_id, h.file_reference);
        offset = match offset.checked_add(len) {
            Some(o) => o,
            None => break,
        };
    }
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

/// Borrow a UTF-16 view directly from little-endian on-disk bytes.
///
/// NTFS stores names as UTF-16LE. This crate is Windows-only, so native
/// `u16` endianness matches the on-disk representation. We still require
/// natural 2-byte alignment before forming a borrowed `&[u16]` view.
fn utf16_slice_from_le_bytes(bytes: &[u8]) -> Option<&[u16]> {
    if !(bytes.as_ptr() as usize).is_multiple_of(2) || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes.len() / 2;
    // SAFETY: the caller provides a byte slice whose length is an exact
    // multiple of 2 and whose start address is 2-byte aligned, so the
    // buffer can be viewed as `[u16]` without reallocating or copying.
    Some(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, units) })
}

/// A view into a single attribute record borrowed from a FILE record's
/// buffer.
pub(crate) struct NtfsAttribute<'a> {
    /// Full attribute-record byte slice.
    data: &'a [u8],
    /// Parsed fixed header copied out of the attribute bytes.
    pub header: NtfsAttributeHeader,
    /// Valid byte length of this attribute record.
    length: usize,
}

impl<'a> NtfsAttribute<'a> {
    /// Read the common attribute header from the start of `data`.
    fn header_from_bytes(data: &[u8]) -> Option<NtfsAttributeHeader> {
        let bytes = data.get(..size_of::<NtfsAttributeHeader>())?;
        NtfsAttributeHeader::read_from_bytes(bytes).ok()
    }

    /// Read the resident attribute header from the start of `data`.
    fn resident_header_from_bytes(data: &[u8]) -> Option<NtfsResidentAttributeHeader> {
        let bytes = data.get(..size_of::<NtfsResidentAttributeHeader>())?;
        NtfsResidentAttributeHeader::read_from_bytes(bytes).ok()
    }

    /// Read the non-resident attribute header from the start of `data`.
    fn nonresident_header_from_bytes(data: &[u8]) -> Option<NtfsNonResidentAttributeHeader> {
        let bytes = data.get(..size_of::<NtfsNonResidentAttributeHeader>())?;
        NtfsNonResidentAttributeHeader::read_from_bytes(bytes).ok()
    }

    /// Validate and borrow an attribute record from the start of `data`.
    pub fn new(data: &'a [u8]) -> Option<Self> {
        let header = Self::header_from_bytes(data)?;
        let length = header.length as usize;
        if length < size_of::<NtfsAttributeHeader>() || length > data.len() {
            return None;
        }
        Some(Self {
            data,
            header,
            length,
        })
    }

    /// Return the validated length of the attribute record.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Return the exact bytes that belong to this attribute record.
    pub fn data(&self) -> &'a [u8] {
        &self.data[..self.length]
    }

    /// Return the raw attribute type code.
    pub fn type_id(&self) -> u32 {
        self.header.type_id
    }

    /// Return whether the attribute is non-resident.
    pub fn is_non_resident(&self) -> bool {
        self.header.is_non_resident != 0
    }

    /// Return the raw on-disk attribute flags.
    pub fn flags(&self) -> u16 {
        self.header.flags
    }

    /// Borrowed UTF-16 view of the attribute name without allocation.
    /// Returns `None` if the attribute is unnamed or the name bytes are
    /// not 2-byte aligned.
    pub fn name_slice(&self) -> Option<&'a [u16]> {
        let n = self.header.name_length as usize;
        if n == 0 {
            return None;
        }
        let off = self.header.name_offset as usize;
        let end = off.checked_add(n.checked_mul(2)?)?;
        if end > self.length {
            return None;
        }
        let bytes = &self.data()[off..end];
        let units = utf16_slice_from_le_bytes(bytes)?;
        debug_assert_eq!(units.len(), n);
        Some(units)
    }

    /// Read the resident header when this attribute is resident.
    pub fn resident_header(&self) -> Option<NtfsResidentAttributeHeader> {
        if self.is_non_resident() {
            return None;
        }
        Self::resident_header_from_bytes(self.data())
    }

    /// Read the non-resident header when this attribute is non-resident.
    pub fn nonresident_header(&self) -> Option<NtfsNonResidentAttributeHeader> {
        if !self.is_non_resident() {
            return None;
        }
        Self::nonresident_header_from_bytes(self.data())
    }

    /// Borrow the resident value payload.
    pub fn resident_value(&self) -> Option<&'a [u8]> {
        let h = self.resident_header()?;
        let start = h.value_offset as usize;
        let end = start.checked_add(h.value_length as usize)?;
        if end > self.length {
            return None;
        }
        Some(&self.data()[start..end])
    }

    /// Interpret the resident payload as `$STANDARD_INFORMATION`.
    pub fn as_standard_info(&self) -> Option<NtfsStandardInformation> {
        if self.type_id() != NtfsAttributeType::StandardInformation as u32 {
            return None;
        }
        let v = self.resident_value()?;
        let bytes = v.get(..size_of::<NtfsStandardInformation>())?;
        NtfsStandardInformation::read_from_bytes(bytes).ok()
    }

    /// Returns `(header, name_utf16_units)` for a `$FILE_NAME` attribute.
    /// The name slice borrows directly from the attribute buffer when
    /// 2-byte aligned; otherwise the attribute is rejected (which is
    /// extremely rare in practice since attribute records start on
    /// 8-byte boundaries).
    pub fn as_file_name(&self) -> Option<(NtfsFileNameHeader, &'a [u16])> {
        if self.type_id() != NtfsAttributeType::FileName as u32 {
            return None;
        }
        let v = self.resident_value()?;
        let header_bytes = v.get(..size_of::<NtfsFileNameHeader>())?;
        let header = NtfsFileNameHeader::read_from_bytes(header_bytes).ok()?;
        let n = header.name_length as usize;
        let needed = size_of::<NtfsFileNameHeader>().checked_add(n.checked_mul(2)?)?;
        if needed > v.len() || n > 255 {
            return None;
        }
        let bytes = &v[size_of::<NtfsFileNameHeader>()..needed];
        let units = utf16_slice_from_le_bytes(bytes)?;
        debug_assert_eq!(units.len(), n);
        Some((header, units))
    }
}

/// Iterate attributes in a FILE record buffer starting at `attrs_offset`
/// up to `used_size`. Skips invalid trailing data; stops at the End
/// marker.
pub(crate) fn for_each_attribute<'a, F>(
    data: &'a [u8],
    attrs_offset: usize,
    used_size: usize,
    mut f: F,
) where
    F: FnMut(&NtfsAttribute<'a>),
{
    let used = used_size.min(data.len());
    let mut offset = attrs_offset;
    while offset + 4 <= used {
        // Peek at type_id first so we can detect the End marker (which
        // typically carries length=0 and would otherwise be rejected by
        // `NtfsAttribute::new`).
        let type_id = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        if type_id == NtfsAttributeType::End as u32 {
            break;
        }
        let slice = &data[offset..used];
        let attr = match NtfsAttribute::new(slice) {
            Some(a) => a,
            None => break,
        };
        f(&attr);
        let len = attr.len();
        if len == 0 {
            break;
        }
        offset = match offset.checked_add(len) {
            Some(n) if n <= used => n,
            _ => break,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_resident_attr(type_id: u32, value: &[u8]) -> Vec<u8> {
        let header_size = size_of::<NtfsResidentAttributeHeader>();
        let total = header_size + value.len();
        let mut buf = vec![0u8; total];
        let header = NtfsResidentAttributeHeader {
            attribute_header: NtfsAttributeHeader {
                type_id,
                length: total as u32,
                is_non_resident: 0,
                name_length: 0,
                name_offset: 0,
                flags: 0,
                id: 0,
            },
            value_length: value.len() as u32,
            value_offset: header_size as u16,
            indexed_flag: 0,
            _pad: 0,
        };
        buf[..header_size].copy_from_slice(header.as_bytes());
        buf[header_size..].copy_from_slice(value);
        buf
    }

    #[test]
    fn parses_standard_information() {
        let si = NtfsStandardInformation {
            creation_time: 1,
            modification_time: 2,
            mft_record_modification_time: 3,
            access_time: 4,
            file_attributes: 0x20,
        };
        let mut value = vec![0u8; size_of::<NtfsStandardInformation>()];
        value.copy_from_slice(si.as_bytes());
        let buf = build_resident_attr(NtfsAttributeType::StandardInformation as u32, &value);
        let attr = NtfsAttribute::new(&buf).expect("attr");
        let parsed = attr.as_standard_info().expect("std info");
        let creation_time = parsed.creation_time;
        let file_attrs = parsed.file_attributes;
        assert_eq!(creation_time, 1);
        assert_eq!(file_attrs, 0x20);
    }

    #[test]
    fn parses_file_name() {
        let name: Vec<u16> = "hello.txt".encode_utf16().collect();
        let header_size = size_of::<NtfsFileNameHeader>();
        let mut value = vec![0u8; header_size + name.len() * 2];
        let h = NtfsFileNameHeader {
            parent_directory_reference: 0x0001_0000_0000_0005,
            creation_time: 0,
            modification_time: 0,
            mft_record_modification_time: 0,
            access_time: 0,
            allocated_size: 4096,
            real_size: 9,
            file_attributes: 0x20,
            reparse_point_tag: 0,
            name_length: name.len() as u8,
            namespace: FileNameNamespace::Win32 as u8,
        };
        value[..header_size].copy_from_slice(h.as_bytes());
        for (i, &u) in name.iter().enumerate() {
            let off = header_size + i * 2;
            value[off..off + 2].copy_from_slice(&u.to_le_bytes());
        }
        let buf = build_resident_attr(NtfsAttributeType::FileName as u32, &value);
        let attr = NtfsAttribute::new(&buf).expect("attr");
        let (parsed, units) = attr.as_file_name().expect("file name");
        let name_len = parsed.name_length;
        assert_eq!(name_len as usize, name.len());
        assert_eq!(units, name.as_slice());
    }

    #[test]
    fn iterates_attributes_until_end() {
        let mut buf = vec![0u8; 512];
        let attrs_offset = 0usize;
        let a1 = build_resident_attr(NtfsAttributeType::StandardInformation as u32, &[0u8; 48]);
        buf[..a1.len()].copy_from_slice(&a1);
        let mut next = a1.len();
        let a2 = build_resident_attr(NtfsAttributeType::Data as u32, &[1, 2, 3, 4]);
        buf[next..next + a2.len()].copy_from_slice(&a2);
        next += a2.len();
        // End marker
        buf[next..next + 4].copy_from_slice(&(NtfsAttributeType::End as u32).to_le_bytes());
        // length=0xFFFFFFFF would also serve; we just use end marker.
        let mut seen = Vec::new();
        for_each_attribute(&buf, attrs_offset, 512, |a| seen.push(a.type_id()));
        assert_eq!(
            seen,
            vec![
                NtfsAttributeType::StandardInformation as u32,
                NtfsAttributeType::Data as u32,
            ]
        );
    }

    #[test]
    fn rejects_attribute_with_zero_length() {
        let mut buf = vec![0u8; 64];
        // type_id = 0x10, length = 0
        buf[0..4].copy_from_slice(&0x10u32.to_le_bytes());
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert!(NtfsAttribute::new(&buf).is_none());
    }
}
