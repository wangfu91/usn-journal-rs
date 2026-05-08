//! Lean batch-oriented raw-MFT types used by the chunk-parallel APIs.
//!
//! These types intentionally carry less data than [`super::RawMftEntry`] so
//! high-throughput batch consumers can avoid rebuilding fields they do not use.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use crate::{Fid, FileAttributes, Filetime};

use super::{
    FileNameNamespace, RawMftEntry, RawMftLink, RawMftWorkChunk,
    attribute::{NtfsAttribute, NtfsAttributeType, file_attr_flags, for_each_attribute},
    entry::AttributeListInfo,
    record::FileRecord,
};

/// Parsed batch result for one logical raw-MFT work chunk.
///
/// The `entries` preserve the record order produced by the underlying chunk
/// scan so callers can fold or compare batches deterministically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMftChunkBatch {
    /// Logical chunk that produced this batch.
    pub chunk: RawMftWorkChunk,
    /// Lean entries parsed from the chunk in chunk order.
    pub entries: Vec<RawMftBatchEntry>,
}

/// Lean raw-MFT entry shape for high-throughput batch consumers.
///
/// Compared with [`super::RawMftEntry`], this form keeps only the metadata
/// needed by the chunk/batch APIs and omits richer fields such as timestamps
/// from `$FILE_NAME`, alternate data stream details, and data-run summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMftBatchEntry {
    /// Record number in the `$MFT`.
    pub record_number: u64,
    /// Full file reference for this record.
    pub file_reference: Fid,
    /// Parent directory file reference from the selected `$FILE_NAME`.
    pub parent_reference: Fid,
    /// Base-record reference for extension records.
    pub base_record_reference: u64,
    /// Whether the record is marked as a directory.
    pub is_directory: bool,
    /// Reparse tag when the file is a reparse point.
    pub reparse_tag: Option<u32>,
    /// Namespace of the selected `$FILE_NAME`.
    pub namespace: FileNameNamespace,
    /// Selected leaf file name.
    pub file_name: OsString,
    /// `$STANDARD_INFORMATION` last-modified timestamp.
    pub si_modified: Filetime,
    /// `$STANDARD_INFORMATION` attribute flags.
    pub si_file_attributes: FileAttributes,
    /// Logical size of the unnamed data stream.
    pub real_size: u64,
    /// Allocated size of the unnamed data stream.
    pub allocated_size: u64,
    /// All `$FILE_NAME` links observed on the record and loaded extensions.
    pub links: Box<[RawMftLink]>,
}

#[derive(Debug)]
pub(crate) struct RawMftBatchScratch {
    pub(crate) entry: RawMftBatchEntry,
    pub(crate) hard_link_count: u16,
    pub(crate) have_unnamed_data: bool,
    pub(crate) is_reparse_point: bool,
}

impl RawMftBatchScratch {
    pub(crate) fn from_record_with_attr_list(
        record: &FileRecord<'_>,
        collect_dos_file_name_links: bool,
    ) -> (Self, Option<AttributeListInfo>) {
        let mut builder = RawMftBatchEntryBuilder::new(record, collect_dos_file_name_links);
        let (attrs_off, used) = record.attrs_range();
        for_each_attribute(record.data, attrs_off, used, |attr| {
            builder.consume_attribute(attr);
        });
        builder.build()
    }

    pub(crate) fn into_entry(self) -> RawMftBatchEntry {
        self.entry
    }
}

impl From<RawMftEntry> for RawMftBatchEntry {
    fn from(entry: RawMftEntry) -> Self {
        Self {
            record_number: entry.record_number,
            file_reference: entry.file_reference,
            parent_reference: entry.parent_reference,
            base_record_reference: entry.base_record_reference,
            is_directory: entry.is_directory,
            reparse_tag: entry.reparse_tag,
            namespace: entry.namespace,
            file_name: entry.file_name,
            si_modified: entry.si_modified,
            si_file_attributes: entry.si_file_attributes,
            real_size: entry.real_size,
            allocated_size: entry.allocated_size,
            links: entry.links,
        }
    }
}

struct RawMftBatchEntryBuilder {
    scratch: RawMftBatchScratch,
    links: Vec<RawMftLink>,
    best_namespace_score: i32,
    attr_list: Option<AttributeListInfo>,
    collect_dos_file_name_links: bool,
}

impl RawMftBatchEntryBuilder {
    fn new(record: &FileRecord<'_>, collect_dos_file_name_links: bool) -> Self {
        Self {
            scratch: RawMftBatchScratch {
                entry: RawMftBatchEntry {
                    record_number: record.number,
                    file_reference: Fid::new(record.file_reference()),
                    parent_reference: Fid::new(0),
                    base_record_reference: record.base_reference() & 0x0000_FFFF_FFFF_FFFF,
                    is_directory: record.is_directory(),
                    reparse_tag: None,
                    namespace: FileNameNamespace::Posix,
                    file_name: OsString::new(),
                    si_modified: Filetime::new(0),
                    si_file_attributes: FileAttributes::empty(),
                    real_size: 0,
                    allocated_size: 0,
                    links: Box::default(),
                },
                hard_link_count: record.link_count(),
                have_unnamed_data: false,
                is_reparse_point: false,
            },
            links: Vec::new(),
            best_namespace_score: -1,
            attr_list: None,
            collect_dos_file_name_links,
        }
    }

    fn consume_attribute(&mut self, attr: &NtfsAttribute<'_>) {
        let type_id = attr.type_id();
        if type_id == NtfsAttributeType::StandardInformation as u32 {
            self.apply_standard_information(attr);
        } else if type_id == NtfsAttributeType::FileName as u32 {
            self.apply_file_name(attr);
        } else if type_id == NtfsAttributeType::Data as u32 {
            self.apply_data_attribute(attr);
        } else if type_id == NtfsAttributeType::ReparsePoint as u32 {
            self.apply_reparse_point_attribute(attr);
        } else if type_id == NtfsAttributeType::AttributeList as u32 {
            self.capture_attribute_list(attr);
        }
    }

    fn capture_attribute_list(&mut self, attr: &NtfsAttribute<'_>) {
        if attr.is_non_resident() {
            if let Some(h) = attr.nonresident_header() {
                let runs_off = h.data_runs_offset as usize;
                let attr_bytes = attr.data();
                if runs_off <= attr_bytes.len() {
                    self.attr_list = Some(AttributeListInfo::NonResident {
                        runs_data: attr_bytes[runs_off..].to_vec(),
                        data_size: h.data_size,
                    });
                }
            }
        } else if let Some(value) = attr.resident_value() {
            self.attr_list = Some(AttributeListInfo::Resident(value.to_vec()));
        }
    }

    fn apply_standard_information(&mut self, attr: &NtfsAttribute<'_>) {
        if let Some(si) = attr.as_standard_info() {
            self.scratch.entry.si_modified = Filetime::new(si.modification_time);
            self.scratch.entry.si_file_attributes =
                FileAttributes::from_bits_retain(si.file_attributes);
        }
    }

    fn apply_file_name(&mut self, attr: &NtfsAttribute<'_>) {
        if let Some((header, name_units)) = attr.as_file_name() {
            let namespace = FileNameNamespace::from_u8(header.namespace);
            let score = namespace.score();
            let parent_reference = Fid::new(header.parent_directory_reference);
            if !self.collect_dos_file_name_links
                && namespace == FileNameNamespace::Dos
                && self.has_non_dos_file_name_link(parent_reference)
            {
                return;
            }
            let file_name = OsString::from_wide(name_units);
            if !self.collect_dos_file_name_links && namespace != FileNameNamespace::Dos {
                self.links.retain(|link| {
                    link.namespace != FileNameNamespace::Dos
                        || link.parent_reference != parent_reference
                });
            }
            if self.best_namespace_score >= 0 {
                if self.links.is_empty()
                    && self.should_retain_file_name_link(
                        self.scratch.entry.namespace,
                        self.scratch.entry.parent_reference,
                        namespace,
                        parent_reference,
                    )
                {
                    self.links.push(RawMftLink {
                        parent_reference: self.scratch.entry.parent_reference,
                        namespace: self.scratch.entry.namespace,
                        file_name: self.scratch.entry.file_name.clone(),
                    });
                }
                if self.should_retain_file_name_link(
                    namespace,
                    parent_reference,
                    namespace,
                    parent_reference,
                ) {
                    self.links.push(RawMftLink {
                        parent_reference,
                        namespace,
                        file_name: file_name.clone(),
                    });
                }
            }
            if score > self.best_namespace_score {
                self.best_namespace_score = score;
                self.scratch.entry.namespace = namespace;
                self.scratch.entry.file_name = file_name;
                self.scratch.entry.parent_reference = parent_reference;
                if header.file_attributes & file_attr_flags::REPARSE_POINT != 0 {
                    self.scratch.is_reparse_point = true;
                    self.scratch.entry.reparse_tag = Some(header.reparse_point_tag);
                }
            }
        }
    }

    fn has_non_dos_file_name_link(&self, parent_reference: Fid) -> bool {
        (self.best_namespace_score >= 0
            && self.scratch.entry.parent_reference == parent_reference
            && self.scratch.entry.namespace != FileNameNamespace::Dos)
            || self.links.iter().any(|link| {
                link.parent_reference == parent_reference && link.namespace != FileNameNamespace::Dos
            })
    }

    fn should_retain_file_name_link(
        &self,
        link_namespace: FileNameNamespace,
        link_parent: Fid,
        current_namespace: FileNameNamespace,
        current_parent: Fid,
    ) -> bool {
        if self.collect_dos_file_name_links || link_namespace != FileNameNamespace::Dos {
            return true;
        }
        let current_shadows_link =
            current_namespace != FileNameNamespace::Dos && current_parent == link_parent;
        let existing_link_shadows = self.links.iter().any(|link| {
            link.parent_reference == link_parent && link.namespace != FileNameNamespace::Dos
        });
        !current_shadows_link && !existing_link_shadows
    }

    fn apply_data_attribute(&mut self, attr: &NtfsAttribute<'_>) {
        let stream_name = attr.name_slice();
        if attr.is_non_resident() {
            if stream_name.is_none()
                && let Some(header) = attr.nonresident_header()
                && !self.scratch.have_unnamed_data
            {
                self.scratch.have_unnamed_data = true;
                self.scratch.entry.real_size = header.data_size;
                self.scratch.entry.allocated_size = header.allocated_size;
            }
            return;
        }
        if stream_name.is_none()
            && let Some(header) = attr.resident_header()
            && !self.scratch.have_unnamed_data
        {
            self.scratch.have_unnamed_data = true;
            self.scratch.entry.real_size = header.value_length as u64;
            self.scratch.entry.allocated_size = header.value_length as u64;
        }
    }

    fn apply_reparse_point_attribute(&mut self, attr: &NtfsAttribute<'_>) {
        let Some(value) = attr.resident_value() else {
            return;
        };
        let Some(tag_bytes) = value.get(..4) else {
            return;
        };
        self.scratch.is_reparse_point = true;
        self.scratch.entry.reparse_tag = Some(u32::from_le_bytes([
            tag_bytes[0],
            tag_bytes[1],
            tag_bytes[2],
            tag_bytes[3],
        ]));
    }

    fn build(mut self) -> (RawMftBatchScratch, Option<AttributeListInfo>) {
        if self.scratch.entry.si_file_attributes.bits() & file_attr_flags::REPARSE_POINT != 0 {
            self.scratch.is_reparse_point = true;
        }
        self.scratch.entry.links = self.links.into_boxed_slice();
        (self.scratch, self.attr_list)
    }
}
