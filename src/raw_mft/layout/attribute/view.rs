//! Borrowing views and typed decoders for individual NTFS attributes.
//!
//! `NtfsAttribute` validates a single attribute record, keeps a compact copy
//! of its common header, and exposes helpers for resident/non-resident
//! payload access plus typed decoding of the attribute payloads the crate
//! currently cares about.

use std::mem::size_of;

use zerocopy::FromBytes;

use super::{
    NtfsAttributeHeader, NtfsAttributeType, NtfsFileNameHeader, NtfsNonResidentAttributeHeader,
    NtfsResidentAttributeHeader, NtfsStandardInformation,
};

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

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use zerocopy::IntoBytes;

    use super::*;
    use crate::raw_mft::layout::attribute::FileNameNamespace;

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
        let file_attributes = parsed.file_attributes;
        assert_eq!(creation_time, 1);
        assert_eq!(file_attributes, 0x20);
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
        assert_eq!(parsed.name_length as usize, name.len());
        assert_eq!(units, name.as_slice());
    }

    #[test]
    fn rejects_attribute_with_zero_length() {
        let mut buf = vec![0u8; 64];
        buf[0..4].copy_from_slice(&0x10u32.to_le_bytes());
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert!(NtfsAttribute::new(&buf).is_none());
    }
}



