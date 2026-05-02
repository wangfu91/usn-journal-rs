//! NTFS attribute decoding.
//!
//! Each FILE record contains a sequence of attribute records preceded by
//! either a resident or non-resident header. This module exposes a
//! lightweight view (`NtfsAttribute`) that borrows from a fixed-up record
//! buffer plus typed accessors for the attributes we care about.

use std::mem::size_of;

/// Attribute type identifiers used by NTFS.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum NtfsAttributeType {
    StandardInformation = 0x10,
    AttributeList = 0x20,
    FileName = 0x30,
    Data = 0x80,
    Bitmap = 0xB0,
    ReparsePoint = 0xC0,
    End = 0xFFFF_FFFF,
}

/// Raw on-disk attribute header common to both resident and non-resident
/// attributes.
#[repr(C, packed)]
pub(crate) struct NtfsAttributeHeader {
    pub type_id: u32,
    pub length: u32,
    pub is_non_resident: u8,
    pub name_length: u8,
    pub name_offset: u16,
    pub flags: u16,
    pub id: u16,
}

#[repr(C, packed)]
pub(crate) struct NtfsResidentAttributeHeader {
    pub attribute_header: NtfsAttributeHeader,
    pub value_length: u32,
    pub value_offset: u16,
    pub indexed_flag: u8,
    pub _pad: u8,
}

#[repr(C, packed)]
pub(crate) struct NtfsNonResidentAttributeHeader {
    pub attribute_header: NtfsAttributeHeader,
    pub lowest_vcn: i64,
    pub highest_vcn: i64,
    pub data_runs_offset: u16,
    pub compression_unit_exponent: u8,
    pub _reserved: [u8; 5],
    pub allocated_size: u64,
    pub data_size: u64,
    pub initialized_size: u64,
}

/// Standard information attribute (`$STANDARD_INFORMATION`, 0x10).
#[repr(C, packed)]
pub(crate) struct NtfsStandardInformation {
    pub creation_time: u64,
    pub modification_time: u64,
    pub mft_record_modification_time: u64,
    pub access_time: u64,
    pub file_attributes: u32,
}

/// File-name attribute namespace.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileNameNamespace {
    Posix = 0,
    Win32 = 1,
    Dos = 2,
    Win32AndDos = 3,
}

impl FileNameNamespace {
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
        // SAFETY: We just verified at least `ATTR_LIST_ENTRY_MIN_SIZE` bytes
        // remain at `offset`.  `AttributeListEntryHeader` is `#[repr(C, packed)]`
        // (alignment 1), so any byte pointer is suitably aligned.
        let h = unsafe { &*(data[offset..].as_ptr() as *const AttributeListEntryHeader) };
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
#[derive(Copy, Clone)]
pub(crate) struct NtfsFileNameHeader {
    pub parent_directory_reference: u64,
    pub creation_time: u64,
    pub modification_time: u64,
    pub mft_record_modification_time: u64,
    pub access_time: u64,
    pub allocated_size: u64,
    pub real_size: u64,
    pub file_attributes: u32,
    pub reparse_point_tag: u32,
    pub name_length: u8,
    pub namespace: u8,
}

/// File-attribute flag bits (FILE_NAME / STANDARD_INFORMATION).
pub mod file_attr_flags {
    pub const SPARSE_FILE: u32 = 0x0200;
    pub const REPARSE_POINT: u32 = 0x0400;
    pub const COMPRESSED: u32 = 0x0800;
    pub const ENCRYPTED: u32 = 0x4000;
}

/// A view into a single attribute record borrowed from a FILE record's
/// buffer.
pub(crate) struct NtfsAttribute<'a> {
    data: &'a [u8],
    pub header: &'a NtfsAttributeHeader,
    length: usize,
}

impl<'a> NtfsAttribute<'a> {
    pub fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < size_of::<NtfsAttributeHeader>() {
            return None;
        }
        // SAFETY: We have just verified `data.len() >= sizeof(NtfsAttributeHeader)`.
        // The header is `#[repr(C, packed)]` (alignment 1), so any byte
        // pointer is suitably aligned to form a reference.
        let header = unsafe { &*(data.as_ptr() as *const NtfsAttributeHeader) };
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

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn data(&self) -> &'a [u8] {
        &self.data[..self.length]
    }

    pub fn type_id(&self) -> u32 {
        self.header.type_id
    }

    pub fn is_non_resident(&self) -> bool {
        self.header.is_non_resident != 0
    }

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
        if !(bytes.as_ptr() as usize).is_multiple_of(2) {
            return None;
        }
        // SAFETY: aligned and length is `n * 2` bytes.
        Some(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, n) })
    }

    pub fn resident_header(&self) -> Option<&'a NtfsResidentAttributeHeader> {
        if self.is_non_resident() {
            return None;
        }
        if self.length < size_of::<NtfsResidentAttributeHeader>() {
            return None;
        }
        // SAFETY: `is_non_resident == 0` (resident) and we just verified
        // `self.length >= sizeof(NtfsResidentAttributeHeader)`. The
        // header is packed (alignment 1).
        Some(unsafe { &*(self.data.as_ptr() as *const NtfsResidentAttributeHeader) })
    }

    pub fn nonresident_header(&self) -> Option<&'a NtfsNonResidentAttributeHeader> {
        if !self.is_non_resident() {
            return None;
        }
        if self.length < size_of::<NtfsNonResidentAttributeHeader>() {
            return None;
        }
        // SAFETY: `is_non_resident != 0` and we just verified
        // `self.length >= sizeof(NtfsNonResidentAttributeHeader)`. The
        // header is packed (alignment 1).
        Some(unsafe { &*(self.data.as_ptr() as *const NtfsNonResidentAttributeHeader) })
    }

    pub fn resident_value(&self) -> Option<&'a [u8]> {
        let h = self.resident_header()?;
        let start = h.value_offset as usize;
        let end = start.checked_add(h.value_length as usize)?;
        if end > self.length {
            return None;
        }
        Some(&self.data()[start..end])
    }

    pub fn as_standard_info(&self) -> Option<&'a NtfsStandardInformation> {
        if self.type_id() != NtfsAttributeType::StandardInformation as u32 {
            return None;
        }
        let v = self.resident_value()?;
        if v.len() < size_of::<NtfsStandardInformation>() {
            return None;
        }
        // SAFETY: We have just verified `v.len() >= sizeof(NtfsStandardInformation)`.
        // The struct is packed (alignment 1).
        Some(unsafe { &*(v.as_ptr() as *const NtfsStandardInformation) })
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
        if v.len() < size_of::<NtfsFileNameHeader>() {
            return None;
        }
        let header = unsafe { std::ptr::read_unaligned(v.as_ptr() as *const NtfsFileNameHeader) };
        let n = header.name_length as usize;
        let needed = size_of::<NtfsFileNameHeader>().checked_add(n.checked_mul(2)?)?;
        if needed > v.len() || n > 255 {
            return None;
        }
        let bytes = &v[size_of::<NtfsFileNameHeader>()..needed];
        if !(bytes.as_ptr() as usize).is_multiple_of(2) {
            // Pathological alignment — extremely unlikely on disk.
            return None;
        }
        // SAFETY: aligned and length is `n * 2` bytes.
        let units = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, n) };
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
        unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr() as *mut NtfsResidentAttributeHeader, header);
        }
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
        unsafe {
            std::ptr::write_unaligned(value.as_mut_ptr() as *mut NtfsStandardInformation, si);
        }
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
        unsafe {
            std::ptr::write_unaligned(value.as_mut_ptr() as *mut NtfsFileNameHeader, h);
        }
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
