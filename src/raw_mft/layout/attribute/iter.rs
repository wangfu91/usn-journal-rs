//! Iteration helpers for walking NTFS attribute records and `$ATTRIBUTE_LIST` entries.
//!
//! These helpers operate on already-bounded byte slices from a fixed-up FILE
//! record and stop cleanly on malformed trailing data instead of panicking or
//! reading past the validated region.

use std::mem::size_of;

use zerocopy::FromBytes;

use super::{AttributeListEntryHeader, NtfsAttribute, NtfsAttributeType};

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
    use zerocopy::IntoBytes;

    use crate::raw_mft::layout::attribute::{NtfsAttributeHeader, NtfsResidentAttributeHeader};

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
    fn iterates_attributes_until_end() {
        let mut buf = vec![0u8; 512];
        let attrs_offset = 0usize;
        let a1 = build_resident_attr(NtfsAttributeType::StandardInformation as u32, &[0u8; 48]);
        buf[..a1.len()].copy_from_slice(&a1);
        let mut next = a1.len();
        let a2 = build_resident_attr(NtfsAttributeType::Data as u32, &[1, 2, 3, 4]);
        buf[next..next + a2.len()].copy_from_slice(&a2);
        next += a2.len();
        buf[next..next + 4].copy_from_slice(&(NtfsAttributeType::End as u32).to_le_bytes());

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
    fn iterates_attribute_list_entry() {
        let header = AttributeListEntryHeader {
            type_id: NtfsAttributeType::FileName as u32,
            record_length: size_of::<AttributeListEntryHeader>() as u16,
            attribute_name_length: 0,
            attribute_name_offset: 0,
            lowest_vcn: 0,
            file_reference: 0x0001_0000_0000_0042,
            attribute_id: 7,
        };
        let mut bytes = vec![0u8; size_of::<AttributeListEntryHeader>()];
        bytes.copy_from_slice(header.as_bytes());

        let mut seen = Vec::new();
        for_each_attr_list_entry(&bytes, |type_id, file_reference| {
            seen.push((type_id, file_reference));
        });

        assert_eq!(
            seen,
            vec![(NtfsAttributeType::FileName as u32, 0x0001_0000_0000_0042)]
        );
    }
}


