//! Lean batch-oriented raw-MFT types used by the chunk-parallel APIs.
//!
//! These types intentionally carry less data than [`super::RawMftEntry`] so
//! high-throughput batch consumers can avoid rebuilding fields they do not use.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use crate::{
    Fid, FileAttributes, Filetime,
    raw_mft::{
        RawMftWorkChunk,
        ondisk::{
            attribute::{FileNameNamespace, NtfsAttribute, file_attr_flags},
            record::FileRecord,
        },
    },
};

use super::{
    capture::resident_reparse_tag,
    entry::{AttributeListInfo, RawMftEntry, RawMftLink},
    fold::{AttributeConsumer, fold_record_attributes},
    names::{FileNameSelector, current_file_name},
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

/// Internal batch-entry shape that keeps the extra state needed during parse
/// and `$ATTRIBUTE_LIST` enrichment.
#[derive(Debug)]
pub(crate) struct RawMftBatchScratch {
    /// Lean batch entry being incrementally built from one FILE record.
    pub(crate) entry: RawMftBatchEntry,
    /// Hard-link count used to decide whether extension records may carry
    /// additional file-name links.
    pub(crate) hard_link_count: u16,
    /// Whether the base record already supplied unnamed-data sizing metadata.
    pub(crate) have_unnamed_data: bool,
    /// Whether the base record advertises a reparse point.
    pub(crate) is_reparse_point: bool,
}

impl RawMftBatchScratch {
    /// Build a lean batch scratch entry plus any captured `$ATTRIBUTE_LIST`
    /// payload from one parsed FILE record.
    pub(crate) fn from_record_with_attr_list(
        record: &FileRecord<'_>,
        collect_dos_file_name_links: bool,
    ) -> (Self, Option<AttributeListInfo>) {
        let mut builder = RawMftBatchEntryBuilder::new(record, collect_dos_file_name_links);
        fold_record_attributes(record, &mut builder);
        builder.build()
    }

    /// Drop scratch-only bookkeeping and return the lean public batch entry.
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
    file_names: FileNameSelector,
    attr_list: Option<AttributeListInfo>,
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
            file_names: FileNameSelector::new(collect_dos_file_name_links),
            attr_list: None,
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
            let parent_reference = Fid::new(header.parent_directory_reference);
            let file_name = OsString::from_wide(name_units);
            let should_replace = self.file_names.consider(
                current_file_name(
                    self.scratch.entry.namespace,
                    self.scratch.entry.parent_reference,
                    &self.scratch.entry.file_name,
                ),
                namespace,
                parent_reference,
                &file_name,
            );
            if should_replace {
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
        let Some(reparse_tag) = resident_reparse_tag(attr) else {
            return;
        };
        self.scratch.is_reparse_point = true;
        self.scratch.entry.reparse_tag = Some(reparse_tag);
    }

    fn build(mut self) -> (RawMftBatchScratch, Option<AttributeListInfo>) {
        if self.scratch.entry.si_file_attributes.bits() & file_attr_flags::REPARSE_POINT != 0 {
            self.scratch.is_reparse_point = true;
        }
        self.scratch.entry.links = self.file_names.into_links();
        (self.scratch, self.attr_list)
    }
}

impl AttributeConsumer for RawMftBatchEntryBuilder {
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
