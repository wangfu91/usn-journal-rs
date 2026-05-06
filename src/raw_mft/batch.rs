use std::ffi::OsString;

use crate::{Fid, FileAttributes, Filetime};

use super::{FileNameNamespace, RawMftEntry, RawMftLink, RawMftWorkChunk};

/// Parsed batch result for one logical raw-MFT work chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMftChunkBatch {
    /// Logical chunk that produced this batch.
    pub chunk: RawMftWorkChunk,
    /// Lean entries parsed from the chunk.
    pub entries: Vec<RawMftBatchEntry>,
}

/// Lean raw-MFT entry shape for high-throughput batch consumers.
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
