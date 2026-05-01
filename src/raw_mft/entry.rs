//! Rich per-record metadata extracted from a single FILE record.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use log::warn;

use crate::{
    Fid,
    path::PathResolvableEntry,
    raw_mft::{
        attribute::{
            file_attr_flags, for_each_attribute, FileNameNamespace, NtfsAttributeType,
        },
        data_run::{summarize_runs, DataRunSummary},
        record::FileRecord,
    },
    time::Filetime,
};

/// Information about a single named alternate data stream (`$DATA`
/// attribute with a non-empty attribute name).
#[derive(Debug, Clone)]
pub struct AdsInfo {
    pub name: OsString,
    pub real_size: u64,
    pub allocated_size: u64,
    pub is_resident: bool,
}

/// Comprehensive metadata for one MFT record.
#[derive(Debug, Clone)]
pub struct RawMftEntry {
    pub record_number: u64,
    pub sequence_number: u16,
    pub file_reference: Fid,
    pub parent_reference: Fid,
    pub base_record_reference: u64,
    pub hard_link_count: u16,
    pub flags: u16,
    pub is_used: bool,
    pub is_directory: bool,
    pub is_reparse_point: bool,
    pub reparse_tag: Option<u32>,
    pub namespace: FileNameNamespace,

    pub file_name: OsString,

    pub si_created: Filetime,
    pub si_modified: Filetime,
    pub si_mft_modified: Filetime,
    pub si_accessed: Filetime,
    pub si_file_attributes: u32,

    pub fn_created: Filetime,
    pub fn_modified: Filetime,
    pub fn_mft_modified: Filetime,
    pub fn_accessed: Filetime,

    pub real_size: u64,
    pub allocated_size: u64,
    pub is_resident: bool,
    pub is_sparse: bool,
    pub is_compressed: bool,
    pub is_encrypted: bool,
    pub data_run_summary: Option<DataRunSummary>,

    pub alternate_data_streams: Vec<AdsInfo>,
}

impl RawMftEntry {
    /// Build a `RawMftEntry` from a parsed FILE record.
    pub(crate) fn from_record(record: &FileRecord<'_>) -> Self {
        let mut entry = RawMftEntry {
            record_number: record.number,
            sequence_number: record.sequence_value(),
            file_reference: Fid::new(record.file_reference()),
            parent_reference: Fid::new(0),
            base_record_reference: record.base_reference() & 0x0000_FFFF_FFFF_FFFF,
            hard_link_count: record.link_count(),
            flags: record.header.flags,
            is_used: record.is_used(),
            is_directory: record.is_directory(),
            is_reparse_point: false,
            reparse_tag: None,
            namespace: FileNameNamespace::Posix,
            file_name: OsString::new(),
            si_created: Filetime::from_u64(0),
            si_modified: Filetime::from_u64(0),
            si_mft_modified: Filetime::from_u64(0),
            si_accessed: Filetime::from_u64(0),
            si_file_attributes: 0,
            fn_created: Filetime::from_u64(0),
            fn_modified: Filetime::from_u64(0),
            fn_mft_modified: Filetime::from_u64(0),
            fn_accessed: Filetime::from_u64(0),
            real_size: 0,
            allocated_size: 0,
            is_resident: true,
            is_sparse: false,
            is_compressed: false,
            is_encrypted: false,
            data_run_summary: None,
            alternate_data_streams: Vec::new(),
        };

        let mut best_namespace_score: i32 = -1;
        let mut have_unnamed_data = false;

        let (attrs_off, used) = record.attrs_range();
        for_each_attribute(record.data, attrs_off, used, |attr| {
            let type_id = attr.type_id();
            if type_id == NtfsAttributeType::StandardInformation as u32 {
                if let Some(si) = attr.as_standard_info() {
                    entry.si_created = Filetime::from_u64(si.creation_time);
                    entry.si_modified = Filetime::from_u64(si.modification_time);
                    entry.si_mft_modified = Filetime::from_u64(si.mft_record_modification_time);
                    entry.si_accessed = Filetime::from_u64(si.access_time);
                    entry.si_file_attributes = si.file_attributes;
                }
            } else if type_id == NtfsAttributeType::FileName as u32 {
                if let Some((header, name_units)) = attr.as_file_name() {
                    let ns = FileNameNamespace::from_u8(header.namespace);
                    let score = match ns {
                        FileNameNamespace::Win32AndDos => 4,
                        FileNameNamespace::Win32 => 3,
                        FileNameNamespace::Posix => 2,
                        FileNameNamespace::Dos => 1,
                    };
                    if score > best_namespace_score {
                        best_namespace_score = score;
                        entry.namespace = ns;
                        entry.file_name = OsString::from_wide(name_units);
                        entry.parent_reference = Fid::new(header.parent_directory_reference);
                        entry.fn_created = Filetime::from_u64(header.creation_time);
                        entry.fn_modified = Filetime::from_u64(header.modification_time);
                        entry.fn_mft_modified =
                            Filetime::from_u64(header.mft_record_modification_time);
                        entry.fn_accessed = Filetime::from_u64(header.access_time);
                        let fa = header.file_attributes;
                        if fa & file_attr_flags::REPARSE_POINT != 0 {
                            entry.is_reparse_point = true;
                            let tag = header.reparse_point_tag;
                            entry.reparse_tag = Some(tag);
                        }
                    }
                }
            } else if type_id == NtfsAttributeType::Data as u32 {
                let stream_name_slice = attr.name_slice();
                if attr.is_non_resident() {
                    if let Some(h) = attr.nonresident_header() {
                        let allocated = h.allocated_size;
                        let data_size = h.data_size;
                        let runs_off = h.data_runs_offset as usize;
                        let attr_data = attr.data();
                        let runs_slice = if runs_off <= attr_data.len() {
                            &attr_data[runs_off..]
                        } else {
                            &[][..]
                        };
                        let summary = match summarize_runs(runs_slice) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                warn!(
                                    "summarize_runs failed for record {}: {e}",
                                    record.number
                                );
                                None
                            }
                        };
                        let attr_flags = attr.flags();
                        let is_compressed = attr_flags & 0x0001 != 0;
                        let is_encrypted = attr_flags & 0x4000 != 0;
                        let is_sparse = attr_flags & 0x8000 != 0;
                        match stream_name_slice {
                            None => {
                                if !have_unnamed_data {
                                    have_unnamed_data = true;
                                    entry.real_size = data_size;
                                    entry.allocated_size = allocated;
                                    entry.is_resident = false;
                                    entry.is_compressed |= is_compressed;
                                    entry.is_encrypted |= is_encrypted;
                                    entry.is_sparse |= is_sparse;
                                    entry.data_run_summary = summary;
                                }
                            }
                            Some(name_units) => {
                                entry.alternate_data_streams.push(AdsInfo {
                                    name: OsString::from_wide(name_units),
                                    real_size: data_size,
                                    allocated_size: allocated,
                                    is_resident: false,
                                });
                            }
                        }
                    }
                } else if let Some(h) = attr.resident_header() {
                    let value_length = h.value_length as u64;
                    match stream_name_slice {
                        None => {
                            if !have_unnamed_data {
                                have_unnamed_data = true;
                                entry.real_size = value_length;
                                entry.allocated_size = value_length;
                                entry.is_resident = true;
                            }
                        }
                        Some(name_units) => {
                            entry.alternate_data_streams.push(AdsInfo {
                                name: OsString::from_wide(name_units),
                                real_size: value_length,
                                allocated_size: value_length,
                                is_resident: true,
                            });
                        }
                    }
                }
            } else if type_id == NtfsAttributeType::AttributeList as u32 && attr.is_non_resident() {
                warn!(
                    "non-resident $ATTRIBUTE_LIST in record {} is not fully supported",
                    record.number
                );
            }
        });

        // SI flags fold-in for sparse/compressed/encrypted (covers some
        // edge cases where the unnamed $DATA flags weren't set).
        if entry.si_file_attributes & file_attr_flags::SPARSE_FILE != 0 {
            entry.is_sparse = true;
        }
        if entry.si_file_attributes & file_attr_flags::COMPRESSED != 0 {
            entry.is_compressed = true;
        }
        if entry.si_file_attributes & file_attr_flags::ENCRYPTED != 0 {
            entry.is_encrypted = true;
        }
        if entry.si_file_attributes & file_attr_flags::REPARSE_POINT != 0 {
            entry.is_reparse_point = true;
        }

        entry
    }

    /// Strongly-typed view of [`RawMftEntry::si_file_attributes`].
    ///
    /// Unknown bits are preserved.
    #[must_use]
    #[inline]
    pub fn si_file_attributes_flags(&self) -> crate::FileAttributes {
        crate::FileAttributes::from_bits_retain(self.si_file_attributes)
    }
}

impl PathResolvableEntry for RawMftEntry {
    fn fid(&self) -> Fid {
        self.file_reference
    }
    fn parent_fid(&self) -> Fid {
        self.parent_reference
    }
    fn file_name(&self) -> &OsString {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_mft::{
        attribute::{
            FileNameNamespace, NtfsAttributeHeader, NtfsAttributeType, NtfsFileNameHeader,
            NtfsResidentAttributeHeader, NtfsStandardInformation,
        },
        record::{flags as record_flags, FileRecord, FileRecordHeader, FILE_RECORD_SIGNATURE},
    };
    use std::mem::size_of;

    fn write_resident_attr(buf: &mut Vec<u8>, type_id: u32, attr_name: &[u16], value: &[u8]) {
        let header_size = size_of::<NtfsResidentAttributeHeader>();
        let name_off = header_size;
        let name_bytes = attr_name.len() * 2;
        let value_off_unaligned = name_off + name_bytes;
        // 8-byte alignment of value start.
        let value_off = value_off_unaligned.div_ceil(8) * 8;
        let total = value_off + value.len();
        let total_aligned = total.div_ceil(8) * 8;
        let start = buf.len();
        buf.resize(start + total_aligned, 0);
        let header = NtfsResidentAttributeHeader {
            attribute_header: NtfsAttributeHeader {
                type_id,
                length: total_aligned as u32,
                is_non_resident: 0,
                name_length: attr_name.len() as u8,
                name_offset: if attr_name.is_empty() { 0 } else { name_off as u16 },
                flags: 0,
                id: 0,
            },
            value_length: value.len() as u32,
            value_offset: value_off as u16,
            indexed_flag: 0,
            _pad: 0,
        };
        unsafe {
            std::ptr::write_unaligned(
                buf[start..].as_mut_ptr() as *mut NtfsResidentAttributeHeader,
                header,
            );
        }
        for (i, &u) in attr_name.iter().enumerate() {
            let off = start + name_off + i * 2;
            buf[off..off + 2].copy_from_slice(&u.to_le_bytes());
        }
        buf[start + value_off..start + value_off + value.len()].copy_from_slice(value);
    }

    fn build_test_record(record_number: u64) -> Vec<u8> {
        let record_size = 1024usize;
        let attrs_offset = 56usize;
        let mut buf = vec![0u8; record_size];
        // USA layout: offset 42, count 3 (sentinel + 2 sectors).
        let header = FileRecordHeader {
            signature: *FILE_RECORD_SIGNATURE,
            update_sequence_offset: 42,
            update_sequence_length: 3,
            logfile_sequence_number: 0,
            sequence_value: 1,
            link_count: 1,
            attributes_offset: attrs_offset as u16,
            flags: record_flags::IN_USE,
            used_size: 0, // patched below
            allocated_size: record_size as u32,
            base_reference: 0,
            next_attribute_id: 0,
        };
        unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr() as *mut FileRecordHeader, header);
        }
        // Sentinel + replacements
        buf[42] = 0xAB;
        buf[43] = 0xCD;
        buf[44] = 0x11;
        buf[45] = 0x22;
        buf[46] = 0x33;
        buf[47] = 0x44;
        buf[510] = 0xAB;
        buf[511] = 0xCD;
        buf[1022] = 0xAB;
        buf[1023] = 0xCD;

        // Build attributes from attrs_offset onwards.
        let mut attrs = Vec::new();
        // $STANDARD_INFORMATION
        let si = NtfsStandardInformation {
            creation_time: 132_000_000_000_000_000,
            modification_time: 132_000_000_010_000_000,
            mft_record_modification_time: 132_000_000_020_000_000,
            access_time: 132_000_000_030_000_000,
            file_attributes: 0x20,
        };
        let mut si_bytes = vec![0u8; size_of::<NtfsStandardInformation>()];
        unsafe {
            std::ptr::write_unaligned(
                si_bytes.as_mut_ptr() as *mut NtfsStandardInformation,
                si,
            );
        }
        write_resident_attr(
            &mut attrs,
            NtfsAttributeType::StandardInformation as u32,
            &[],
            &si_bytes,
        );
        // $FILE_NAME (Win32 namespace)
        let name: Vec<u16> = "hello.txt".encode_utf16().collect();
        let fn_header = NtfsFileNameHeader {
            parent_directory_reference: (5u64 << 48) | 5,
            creation_time: 132_000_000_000_000_000,
            modification_time: 132_000_000_010_000_000,
            mft_record_modification_time: 132_000_000_020_000_000,
            access_time: 132_000_000_030_000_000,
            allocated_size: 4096,
            real_size: 9,
            file_attributes: 0x20,
            reparse_point_tag: 0,
            name_length: name.len() as u8,
            namespace: FileNameNamespace::Win32 as u8,
        };
        let mut fn_bytes = vec![0u8; size_of::<NtfsFileNameHeader>() + name.len() * 2];
        unsafe {
            std::ptr::write_unaligned(
                fn_bytes.as_mut_ptr() as *mut NtfsFileNameHeader,
                fn_header,
            );
        }
        for (i, &u) in name.iter().enumerate() {
            let off = size_of::<NtfsFileNameHeader>() + i * 2;
            fn_bytes[off..off + 2].copy_from_slice(&u.to_le_bytes());
        }
        write_resident_attr(
            &mut attrs,
            NtfsAttributeType::FileName as u32,
            &[],
            &fn_bytes,
        );
        // resident $DATA
        write_resident_attr(
            &mut attrs,
            NtfsAttributeType::Data as u32,
            &[],
            b"abc12345",
        );
        // named $DATA (alternate stream)
        let ads_name: Vec<u16> = "ads".encode_utf16().collect();
        write_resident_attr(
            &mut attrs,
            NtfsAttributeType::Data as u32,
            &ads_name,
            b"alt",
        );
        // End marker
        attrs.extend_from_slice(&(NtfsAttributeType::End as u32).to_le_bytes());
        attrs.extend_from_slice(&0u32.to_le_bytes());

        let used_size = attrs_offset + attrs.len();
        // copy attrs in
        buf[attrs_offset..attrs_offset + attrs.len()].copy_from_slice(&attrs);
        // patch used_size in header (offset 24, 4 bytes)
        buf[24..28].copy_from_slice(&(used_size as u32).to_le_bytes());

        let _ = record_number;
        buf
    }

    #[test]
    fn builds_entry_from_synthetic_record() {
        let mut buf = build_test_record(42);
        let rec = FileRecord::parse(42, &mut buf).expect("parse");
        let entry = RawMftEntry::from_record(&rec);
        assert_eq!(entry.record_number, 42);
        assert_eq!(entry.file_name.to_string_lossy(), "hello.txt");
        assert_eq!(entry.namespace, FileNameNamespace::Win32);
        assert!(entry.is_used);
        assert!(!entry.is_directory);
        assert_eq!(entry.real_size, 8); // resident $DATA value len
        assert!(entry.is_resident);
        assert_eq!(entry.alternate_data_streams.len(), 1);
        assert_eq!(
            entry.alternate_data_streams[0].name.to_string_lossy(),
            "ads"
        );
        assert_eq!(entry.alternate_data_streams[0].real_size, 3);
        assert!(entry.si_created.as_u64() != 0);
        assert_eq!(entry.parent_reference, Fid::new((5u64 << 48) | 5));
    }

    #[test]
    fn path_resolvable_returns_unmasked_fids() {
        let mut buf = build_test_record(42);
        let rec = FileRecord::parse(42, &mut buf).expect("parse");
        let entry = RawMftEntry::from_record(&rec);
        // file_reference = (seq << 48) | record_number; with seq=1, record=42
        assert_eq!(entry.fid(), Fid::new((1u64 << 48) | 42));
        // parent_directory_reference was built as (5 << 48) | 5
        assert_eq!(entry.parent_fid(), Fid::new((5u64 << 48) | 5));
        assert_eq!(entry.file_name(), &OsString::from("hello.txt"));
        assert!(!entry.is_dir());
    }
}
