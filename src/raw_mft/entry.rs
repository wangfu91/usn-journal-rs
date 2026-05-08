//! Rich per-record metadata extracted from a single FILE record.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use log::warn;

use crate::{
    Fid, FileAttributes, Filetime,
    file_attributes::FileAttributeView,
    path::PathResolvableEntry,
    raw_mft::{
        attribute_capture::resident_reparse_tag,
        attribute_fold::{AttributeConsumer, fold_record_attributes},
        name_selection::{FileNameSelector, current_file_name},
        ondisk::{
            attribute::{FileNameNamespace, NtfsAttribute, file_attr_flags},
            data_run::{DataRunSummary, summarize_runs},
            record::FileRecord,
        },
    },
};

#[derive(Clone, Copy)]
pub(crate) struct EntryBuildOptions {
    pub(crate) collect_alternate_data_streams: bool,
    pub(crate) collect_data_run_summary: bool,
    pub(crate) collect_dos_file_name_links: bool,
}

impl EntryBuildOptions {
    pub(crate) const fn full() -> Self {
        Self {
            collect_alternate_data_streams: true,
            collect_data_run_summary: true,
            collect_dos_file_name_links: true,
        }
    }
}

/// Raw `$ATTRIBUTE_LIST` data captured from a FILE record, used by
/// `from_record_with_attr_list` so the caller can load extension records
/// and enrich the entry with the best-namespace `$FILE_NAME`.
///
/// Extension records exist when a file's attribute set overflows a single
/// 1 KiB FILE record (e.g. a file with many hard links).
#[derive(Debug)]
pub(crate) enum AttributeListInfo {
    /// The attribute list fits inside the FILE record (resident).
    /// Contains the raw value bytes.
    Resident(Vec<u8>),
    /// The attribute list is stored in data runs on disk (non-resident).
    /// `runs_data` is the raw run-length encoded bytes starting at
    /// `data_runs_offset`; `data_size` is the total logical size.
    NonResident { runs_data: Vec<u8>, data_size: u64 },
}

/// Information about a single named alternate data stream (`$DATA`
/// attribute with a non-empty attribute name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdsInfo {
    /// Alternate data stream name.
    pub name: OsString,
    /// Logical stream size in bytes.
    pub real_size: u64,
    /// Allocated stream size in bytes.
    pub allocated_size: u64,
    /// Whether the stream data is resident in the FILE record.
    pub is_resident: bool,
}

/// One `$FILE_NAME` link carried by an MFT record.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawMftLink {
    /// Parent directory file reference for this link.
    pub parent_reference: Fid,
    /// Namespace of this specific `$FILE_NAME` attribute.
    pub namespace: FileNameNamespace,
    /// Leaf name carried by this link.
    pub file_name: OsString,
}

/// Comprehensive metadata for one MFT record.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawMftEntry {
    /// Record number in the `$MFT`.
    pub record_number: u64,
    /// Record sequence number.
    pub sequence_number: u16,
    /// Full file reference for this record.
    pub file_reference: Fid,
    /// Parent directory file reference from `$FILE_NAME`.
    pub parent_reference: Fid,
    /// Base-record reference for extension records.
    pub base_record_reference: u64,
    /// Number of hard links to the file.
    pub hard_link_count: u16,
    /// Raw FILE-record flags.
    pub flags: u16,
    /// Whether the record is marked in use.
    pub is_used: bool,
    /// Whether the record is marked as a directory.
    pub is_directory: bool,
    /// Whether the file is marked as a reparse point.
    pub is_reparse_point: bool,
    /// Reparse tag when the file is a reparse point.
    pub reparse_tag: Option<u32>,
    /// Namespace of the chosen `$FILE_NAME` attribute.
    pub namespace: FileNameNamespace,

    /// Leaf file name.
    pub file_name: OsString,

    /// `$STANDARD_INFORMATION` creation timestamp.
    pub si_created: Filetime,
    /// `$STANDARD_INFORMATION` last-modified timestamp.
    pub si_modified: Filetime,
    /// `$STANDARD_INFORMATION` MFT-record-modified timestamp.
    pub si_mft_modified: Filetime,
    /// `$STANDARD_INFORMATION` last-access timestamp.
    pub si_accessed: Filetime,
    /// `$STANDARD_INFORMATION` file-attribute flags.
    pub si_file_attributes: FileAttributes,

    /// `$FILE_NAME` creation timestamp.
    pub fn_created: Filetime,
    /// `$FILE_NAME` last-modified timestamp.
    pub fn_modified: Filetime,
    /// `$FILE_NAME` MFT-record-modified timestamp.
    pub fn_mft_modified: Filetime,
    /// `$FILE_NAME` last-access timestamp.
    pub fn_accessed: Filetime,

    /// Logical size of the unnamed data stream.
    pub real_size: u64,
    /// Allocated size of the unnamed data stream.
    pub allocated_size: u64,
    /// Whether the record had an unnamed `$DATA` attribute in the parsed record set.
    pub has_unnamed_data: bool,
    /// Whether the unnamed data stream is resident.
    pub is_resident: bool,
    /// Whether the unnamed data stream is sparse.
    pub is_sparse: bool,
    /// Whether the unnamed data stream is compressed.
    pub is_compressed: bool,
    /// Whether the unnamed data stream is encrypted.
    pub is_encrypted: bool,
    /// Summary of the unnamed data stream's runs when non-resident.
    pub data_run_summary: Option<DataRunSummary>,

    /// Named alternate data streams attached to the record.
    pub alternate_data_streams: Box<[AdsInfo]>,
    /// All `$FILE_NAME` links observed on this record and any loaded extension records.
    pub links: Box<[RawMftLink]>,
}

impl RawMftEntry {
    /// Build a `RawMftEntry` and also return any `$ATTRIBUTE_LIST` data
    /// found in the record.
    ///
    /// When `Some(AttributeListInfo)` is returned the caller should parse
    /// the list, find `$FILE_NAME` attribute entries that live in
    /// extension records, load those records, and update the entry with
    /// the highest-scoring file-name namespace.
    pub(crate) fn from_record_with_attr_list(
        record: &FileRecord<'_>,
        options: EntryBuildOptions,
    ) -> (Self, Option<AttributeListInfo>) {
        let mut builder = RawMftEntryBuilder::new(record, options);
        fold_record_attributes(record, &mut builder);
        builder.build()
    }
}

struct RawMftEntryBuilder {
    /// Partially built result entry.
    entry: RawMftEntry,
    /// Alternate streams accumulated while walking attributes.
    alternate_data_streams: Vec<AdsInfo>,
    /// Shared `$FILE_NAME` selection and link-retention state.
    file_names: FileNameSelector,
    /// Whether the unnamed `$DATA` attribute has already been consumed.
    have_unnamed_data: bool,
    /// Captured `$ATTRIBUTE_LIST` data, if any.
    attr_list: Option<AttributeListInfo>,
    /// Whether to collect alternate data stream names and sizes.
    collect_alternate_data_streams: bool,
    /// Whether to compute non-resident data run summaries.
    collect_data_run_summary: bool,
}

impl RawMftEntryBuilder {
    /// Start building an entry from a validated FILE record.
    fn new(record: &FileRecord<'_>, options: EntryBuildOptions) -> Self {
        Self {
            entry: RawMftEntry {
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
                si_created: Filetime::new(0),
                si_modified: Filetime::new(0),
                si_mft_modified: Filetime::new(0),
                si_accessed: Filetime::new(0),
                si_file_attributes: FileAttributes::empty(),
                fn_created: Filetime::new(0),
                fn_modified: Filetime::new(0),
                fn_mft_modified: Filetime::new(0),
                fn_accessed: Filetime::new(0),
                real_size: 0,
                allocated_size: 0,
                has_unnamed_data: false,
                is_resident: true,
                is_sparse: false,
                is_compressed: false,
                is_encrypted: false,
                data_run_summary: None,
                alternate_data_streams: Box::default(),
                links: Box::default(),
            },
            alternate_data_streams: Vec::new(),
            file_names: FileNameSelector::new(options.collect_dos_file_name_links),
            have_unnamed_data: false,
            attr_list: None,
            collect_alternate_data_streams: options.collect_alternate_data_streams,
            collect_data_run_summary: options.collect_data_run_summary,
        }
    }

    /// Fold `$STANDARD_INFORMATION` values into the entry.
    fn apply_standard_information(&mut self, attr: &NtfsAttribute<'_>) {
        if let Some(si) = attr.as_standard_info() {
            self.entry.si_created = Filetime::new(si.creation_time);
            self.entry.si_modified = Filetime::new(si.modification_time);
            self.entry.si_mft_modified = Filetime::new(si.mft_record_modification_time);
            self.entry.si_accessed = Filetime::new(si.access_time);
            self.entry.si_file_attributes = FileAttributes::from_bits_retain(si.file_attributes);
        }
    }

    /// Fold a `$FILE_NAME` attribute into the entry when it has the best namespace so far.
    fn apply_file_name(&mut self, attr: &NtfsAttribute<'_>) {
        if let Some((header, name_units)) = attr.as_file_name() {
            let ns = FileNameNamespace::from_u8(header.namespace);
            let parent_reference = Fid::new(header.parent_directory_reference);
            let file_name = OsString::from_wide(name_units);
            let should_replace = self.file_names.consider(
                current_file_name(
                    self.entry.namespace,
                    self.entry.parent_reference,
                    &self.entry.file_name,
                ),
                ns,
                parent_reference,
                &file_name,
            );
            if should_replace {
                self.entry.namespace = ns;
                self.entry.file_name = file_name;
                self.entry.parent_reference = parent_reference;
                self.entry.fn_created = Filetime::new(header.creation_time);
                self.entry.fn_modified = Filetime::new(header.modification_time);
                self.entry.fn_mft_modified = Filetime::new(header.mft_record_modification_time);
                self.entry.fn_accessed = Filetime::new(header.access_time);
                let fa = header.file_attributes;
                if fa & file_attr_flags::REPARSE_POINT != 0 {
                    self.entry.is_reparse_point = true;
                    self.entry.reparse_tag = Some(header.reparse_point_tag);
                }
            }
        }
    }

    /// Fold a `$DATA` attribute into the unnamed stream or ADS list.
    fn apply_data_attribute(&mut self, attr: &NtfsAttribute<'_>) {
        let stream_name = attr.name_slice();
        if attr.is_non_resident() {
            if let Some(h) = attr.nonresident_header() {
                self.apply_nonresident_data(stream_name, h.allocated_size, h.data_size, attr);
            }
            return;
        }
        if let Some(h) = attr.resident_header() {
            self.apply_resident_data(stream_name, h.value_length as u64);
        }
    }

    /// Fold a non-resident `$DATA` attribute into the entry.
    fn apply_nonresident_data(
        &mut self,
        stream_name: Option<&[u16]>,
        allocated_size: u64,
        data_size: u64,
        attr: &NtfsAttribute<'_>,
    ) {
        let summary = if self.collect_data_run_summary {
            self.nonresident_data_run_summary(attr)
        } else {
            None
        };
        let attr_flags = attr.flags();
        let is_compressed = attr_flags & 0x0001 != 0;
        let is_encrypted = attr_flags & 0x4000 != 0;
        let is_sparse = attr_flags & 0x8000 != 0;
        match stream_name {
            None => {
                if !self.have_unnamed_data {
                    self.have_unnamed_data = true;
                    self.entry.real_size = data_size;
                    self.entry.allocated_size = allocated_size;
                    self.entry.has_unnamed_data = true;
                    self.entry.is_resident = false;
                    self.entry.is_compressed |= is_compressed;
                    self.entry.is_encrypted |= is_encrypted;
                    self.entry.is_sparse |= is_sparse;
                    self.entry.data_run_summary = summary;
                }
            }
            Some(name_units) => {
                if self.collect_alternate_data_streams {
                    self.alternate_data_streams.push(AdsInfo {
                        name: OsString::from_wide(name_units),
                        real_size: data_size,
                        allocated_size,
                        is_resident: false,
                    });
                }
            }
        }
    }

    /// Fold a resident `$REPARSE_POINT` attribute into the entry.
    fn apply_reparse_point_attribute(&mut self, attr: &NtfsAttribute<'_>) {
        let Some(reparse_tag) = resident_reparse_tag(attr) else {
            return;
        };
        self.entry.is_reparse_point = true;
        self.entry.reparse_tag = Some(reparse_tag);
    }

    /// Fold a resident `$DATA` attribute into the entry.
    fn apply_resident_data(&mut self, stream_name: Option<&[u16]>, value_length: u64) {
        match stream_name {
            None => {
                if !self.have_unnamed_data {
                    self.have_unnamed_data = true;
                    self.entry.real_size = value_length;
                    self.entry.allocated_size = value_length;
                    self.entry.has_unnamed_data = true;
                    self.entry.is_resident = true;
                }
            }
            Some(name_units) => {
                if self.collect_alternate_data_streams {
                    self.alternate_data_streams.push(AdsInfo {
                        name: OsString::from_wide(name_units),
                        real_size: value_length,
                        allocated_size: value_length,
                        is_resident: true,
                    });
                }
            }
        }
    }

    /// Summarize the data runs of a non-resident `$DATA` attribute.
    fn nonresident_data_run_summary(&self, attr: &NtfsAttribute<'_>) -> Option<DataRunSummary> {
        let h = attr.nonresident_header()?;
        let runs_off = h.data_runs_offset as usize;
        let attr_data = attr.data();
        let runs_slice = if runs_off <= attr_data.len() {
            &attr_data[runs_off..]
        } else {
            &[][..]
        };
        match summarize_runs(runs_slice) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!(
                    "summarize_runs failed for record {}: {e}",
                    self.entry.record_number
                );
                None
            }
        }
    }

    /// Finalize the entry and return any captured `$ATTRIBUTE_LIST`.
    fn build(mut self) -> (RawMftEntry, Option<AttributeListInfo>) {
        self.fold_si_flags();
        self.entry.alternate_data_streams = self.alternate_data_streams.into_boxed_slice();
        self.entry.links = self.file_names.into_links();
        (self.entry, self.attr_list)
    }

    /// Fold `$STANDARD_INFORMATION` bits into the derived convenience flags.
    fn fold_si_flags(&mut self) {
        // SI flags fold-in for sparse/compressed/encrypted (covers some
        // edge cases where the unnamed $DATA flags weren't set).
        let si_bits = self.entry.si_file_attributes.bits();
        if si_bits & file_attr_flags::SPARSE_FILE != 0 {
            self.entry.is_sparse = true;
        }
        if si_bits & file_attr_flags::COMPRESSED != 0 {
            self.entry.is_compressed = true;
        }
        if si_bits & file_attr_flags::ENCRYPTED != 0 {
            self.entry.is_encrypted = true;
        }
        if si_bits & file_attr_flags::REPARSE_POINT != 0 {
            self.entry.is_reparse_point = true;
        }
    }
}

impl AttributeConsumer for RawMftEntryBuilder {
    fn on_standard_information(&mut self, attr: &NtfsAttribute<'_>) {
        self.apply_standard_information(attr);
    }

    fn on_file_name(&mut self, attr: &NtfsAttribute<'_>) {
        self.apply_file_name(attr);
    }

    fn on_data(&mut self, attr: &NtfsAttribute<'_>) {
        self.apply_data_attribute(attr);
    }

    fn on_reparse_point(&mut self, attr: &NtfsAttribute<'_>) {
        self.apply_reparse_point_attribute(attr);
    }

    fn on_attribute_list(&mut self, attr_list: AttributeListInfo) {
        self.attr_list = Some(attr_list);
    }
}

impl FileAttributeView for RawMftEntry {
    fn file_attributes(&self) -> FileAttributes {
        self.si_file_attributes
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
    use crate::raw_mft::ondisk::{
        attribute::{
            FileNameNamespace, NtfsAttributeHeader, NtfsAttributeType, NtfsFileNameHeader,
            NtfsResidentAttributeHeader, NtfsStandardInformation,
        },
        record::{FILE_RECORD_SIGNATURE, FileRecord, FileRecordHeader, flags as record_flags},
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
                name_offset: if attr_name.is_empty() {
                    0
                } else {
                    name_off as u16
                },
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
            std::ptr::write_unaligned(si_bytes.as_mut_ptr() as *mut NtfsStandardInformation, si);
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
            std::ptr::write_unaligned(fn_bytes.as_mut_ptr() as *mut NtfsFileNameHeader, fn_header);
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
        write_resident_attr(&mut attrs, NtfsAttributeType::Data as u32, &[], b"abc12345");
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
        let rec = FileRecord::parse(42, None, &mut buf).expect("parse");
        let (entry, _) = RawMftEntry::from_record_with_attr_list(&rec, EntryBuildOptions::full());
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
        assert!(entry.si_created.raw() != 0);
        assert_eq!(entry.parent_reference, Fid::new((5u64 << 48) | 5));
        assert!(entry.links.is_empty());
        assert_eq!(entry.parent_reference, Fid::new((5u64 << 48) | 5));
        assert_eq!(entry.file_name.to_string_lossy(), "hello.txt");
    }

    #[test]
    fn path_resolvable_returns_unmasked_fids() {
        let mut buf = build_test_record(42);
        let rec = FileRecord::parse(42, None, &mut buf).expect("parse");
        let (entry, _) = RawMftEntry::from_record_with_attr_list(&rec, EntryBuildOptions::full());
        // file_reference = (seq << 48) | record_number; with seq=1, record=42
        assert_eq!(entry.fid(), Fid::new((1u64 << 48) | 42));
        // parent_directory_reference was built as (5 << 48) | 5
        assert_eq!(entry.parent_fid(), Fid::new((5u64 << 48) | 5));
        assert_eq!(entry.file_name(), &OsString::from("hello.txt"));
        assert!(!entry.is_dir());
    }
}
