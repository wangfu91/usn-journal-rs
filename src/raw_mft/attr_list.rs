//! One-level `$ATTRIBUTE_LIST` enrichment for raw and batch MFT entries.
//!
//! NTFS may spill important attributes into extension records when the base
//! FILE record runs out of room. This module decides when that extra work is
//! worthwhile, materializes the flat attribute list, loads the referenced
//! extension records, and merges the relevant metadata back into the caller's
//! base entry.

use std::ffi::OsStr;

use log::{debug, warn};
use rustc_hash::FxHashSet;

use crate::{
    Fid,
    errors::UsnError,
    raw_mft::{
        entry_build::{
            AttributeListInfo, EntryBuildOptions, RawMftBatchScratch, RawMftEntry, RawMftLink,
        },
        io::VolumeReader,
        ondisk::{
            attribute::{FileNameNamespace, NtfsAttributeType, for_each_attr_list_entry},
            boot::BootSector,
            extent::ExtentMap,
        },
        reader::{read_batch_record_raw, read_nonresident, read_record_raw},
    },
};

/// Counters collected while enriching one base record from its `$ATTRIBUTE_LIST`.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AttrListEnrichStats {
    /// Number of unique extension-record references discovered in the list.
    pub(super) extension_records_referenced: u64,
    /// Number of referenced extension records that were successfully loaded.
    pub(super) extension_records_loaded: u64,
}

/// The categories of metadata a base record is still missing.
#[derive(Debug, Clone, Copy, Default)]
struct EnrichmentNeeds {
    /// The base record needs a better file name or additional hard-link names.
    file_name: bool,
    /// The base record needs unnamed-data metadata or ADS details.
    data: bool,
    /// The base record needs a missing reparse tag.
    reparse: bool,
}

impl EnrichmentNeeds {
    /// Whether any enrichment work remains for the current base record.
    fn any(self) -> bool {
        self.file_name || self.data || self.reparse
    }

    /// Whether the current needs set is interested in a given attribute type.
    fn wants_type(self, type_id: u32) -> bool {
        (self.file_name && type_id == NtfsAttributeType::FileName as u32)
            || (self.data && type_id == NtfsAttributeType::Data as u32)
            || (self.reparse && type_id == NtfsAttributeType::ReparsePoint as u32)
    }
}

/// Borrowed view of the link-related name data carried by one entry.
#[derive(Debug, Clone, Copy)]
struct LinkView<'a> {
    /// Existing boxed links, if the entry already materialized them.
    links: &'a [RawMftLink],
    /// Parent file reference paired with the inline file name.
    parent_reference: Fid,
    /// Namespace paired with the inline file name.
    namespace: FileNameNamespace,
    /// Inline file name used when the entry has no boxed link list yet.
    file_name: &'a OsStr,
}

/// Return `true` when a raw entry should consult its `$ATTRIBUTE_LIST`.
pub(super) fn should_enrich_from_attr_list(entry: &RawMftEntry) -> bool {
    raw_entry_enrichment_needs(entry).any()
}

/// Enrich a raw entry from one level of extension records referenced by its
/// `$ATTRIBUTE_LIST`.
pub(super) fn enrich_from_attr_list(
    entry: &mut RawMftEntry,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    build_options: EntryBuildOptions,
) -> AttrListEnrichStats {
    let needs = raw_entry_enrichment_needs(entry);
    if !needs.any() {
        return AttrListEnrichStats::default();
    }

    let mut best_score = if entry.file_name.is_empty() {
        -1
    } else {
        entry.namespace.score()
    };

    with_loaded_extension_records(
        attr_list,
        base_record_number,
        reader,
        boot.cluster_size,
        |type_id| needs.wants_type(type_id),
        |reader, ext_num| {
            read_record_raw(reader, boot, extent_map, ext_num, build_options)
                .map(|result| result.map(|(entry, _)| entry))
        },
        |ext_entry| {
            if needs.file_name {
                merge_extension_links(entry, &ext_entry);
                if needs.data || needs.reparse {
                    merge_extension_data(entry, &ext_entry, needs);
                }
                let score = ext_entry.namespace.score();
                if score > best_score {
                    best_score = score;
                    entry.namespace = ext_entry.namespace;
                    entry.file_name = ext_entry.file_name;
                    entry.parent_reference = ext_entry.parent_reference;
                    entry.fn_created = ext_entry.fn_created;
                    entry.fn_modified = ext_entry.fn_modified;
                    entry.fn_mft_modified = ext_entry.fn_mft_modified;
                    entry.fn_accessed = ext_entry.fn_accessed;
                }
            } else if needs.data || needs.reparse {
                merge_extension_data(entry, &ext_entry, needs);
            }
        },
    )
}

/// Return `true` when a batch scratch entry should consult its
/// `$ATTRIBUTE_LIST`.
pub(super) fn should_enrich_batch_from_attr_list(entry: &RawMftBatchScratch) -> bool {
    batch_entry_enrichment_needs(entry).any()
}

/// Enrich a batch scratch entry from one level of extension records.
pub(super) fn enrich_batch_from_attr_list(
    entry: &mut RawMftBatchScratch,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    collect_dos_file_name_links: bool,
) -> AttrListEnrichStats {
    let needs = batch_entry_enrichment_needs(entry);
    if !needs.any() {
        return AttrListEnrichStats::default();
    }

    let mut best_score = if entry.entry.file_name.is_empty() {
        -1
    } else {
        entry.entry.namespace.score()
    };

    with_loaded_extension_records(
        attr_list,
        base_record_number,
        reader,
        boot.cluster_size,
        |type_id| needs.wants_type(type_id),
        |reader, ext_num| {
            read_batch_record_raw(
                reader,
                boot,
                extent_map,
                ext_num,
                collect_dos_file_name_links,
            )
            .map(|result| result.map(|(entry, _)| entry))
        },
        |ext_entry| {
            if needs.file_name {
                merge_batch_extension_links(entry, &ext_entry);
                if needs.data || needs.reparse {
                    merge_batch_extension_data(entry, &ext_entry, needs);
                }
                let score = ext_entry.entry.namespace.score();
                if score > best_score {
                    best_score = score;
                    entry.entry.namespace = ext_entry.entry.namespace;
                    entry.entry.file_name = ext_entry.entry.file_name;
                    entry.entry.parent_reference = ext_entry.entry.parent_reference;
                }
            } else if needs.data || needs.reparse {
                merge_batch_extension_data(entry, &ext_entry, needs);
            }
        },
    )
}

/// Materialize a flat `$ATTRIBUTE_LIST` payload from either its resident or
/// non-resident form.
fn materialize_attr_list(
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    cluster_size: u64,
) -> Option<Vec<u8>> {
    match attr_list {
        AttributeListInfo::Resident(bytes) => Some(bytes),
        AttributeListInfo::NonResident {
            runs_data,
            data_size,
        } => {
            let runs = match crate::raw_mft::ondisk::data_run::decode_runs(&runs_data) {
                Ok((runs, _)) => runs,
                Err(error) => {
                    warn!(
                        "raw_mft: record {base_record_number}: \
                         failed to decode $ATTRIBUTE_LIST data runs: {error}"
                    );
                    return None;
                }
            };
            match read_nonresident(reader, &runs, cluster_size, data_size) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    warn!(
                        "raw_mft: record {base_record_number}: \
                         failed to read non-resident $ATTRIBUTE_LIST: {error}"
                    );
                    None
                }
            }
        }
    }
}

/// Load and visit each referenced extension record that matches the caller's
/// current enrichment needs.
fn with_loaded_extension_records<T, WantsType, LoadExtension, VisitExtension>(
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    cluster_size: u64,
    wants_type: WantsType,
    mut load_extension: LoadExtension,
    mut visit_extension: VisitExtension,
) -> AttrListEnrichStats
where
    WantsType: Fn(u32) -> bool,
    LoadExtension: FnMut(&mut VolumeReader, u64) -> Result<Option<T>, UsnError>,
    VisitExtension: FnMut(T),
{
    let mut stats = AttrListEnrichStats::default();
    let data = match materialize_attr_list(attr_list, base_record_number, reader, cluster_size) {
        Some(data) => data,
        None => return stats,
    };

    let ext_records = collect_extension_records(&data, base_record_number, wants_type);
    stats.extension_records_referenced = ext_records.len() as u64;

    for ext_num in ext_records {
        let ext_entry = match load_extension(reader, ext_num) {
            Ok(Some(entry)) => {
                stats.extension_records_loaded += 1;
                entry
            }
            Ok(None) => continue,
            Err(error) => {
                debug!(
                    "raw_mft: record {base_record_number}: \
                     failed to load extension record {ext_num}: {error}"
                );
                continue;
            }
        };
        visit_extension(ext_entry);
    }

    stats
}

/// Extract unique extension-record numbers from a materialized
/// `$ATTRIBUTE_LIST` payload.
fn collect_extension_records<F>(data: &[u8], base_record_number: u64, wants_type: F) -> Vec<u64>
where
    F: Fn(u32) -> bool,
{
    let mut ext_records = Vec::new();
    let mut seen_ext_records = FxHashSet::default();
    for_each_attr_list_entry(data, |type_id, file_ref| {
        if wants_type(type_id) {
            let rec_num = file_ref & 0x0000_FFFF_FFFF_FFFF;
            if rec_num != base_record_number && seen_ext_records.insert(rec_num) {
                ext_records.push(rec_num);
            }
        }
    });
    ext_records
}

/// Compute the metadata categories missing from a rich raw entry.
fn raw_entry_enrichment_needs(entry: &RawMftEntry) -> EnrichmentNeeds {
    EnrichmentNeeds {
        file_name: entry.file_name.is_empty()
            || !matches!(
                entry.namespace,
                FileNameNamespace::Win32 | FileNameNamespace::Win32AndDos
            )
            || entry.hard_link_count > 1,
        data: !entry.is_directory && !entry.has_unnamed_data,
        reparse: entry.is_reparse_point && entry.reparse_tag.is_none(),
    }
}

/// Compute the metadata categories missing from a lean batch scratch entry.
fn batch_entry_enrichment_needs(entry: &RawMftBatchScratch) -> EnrichmentNeeds {
    EnrichmentNeeds {
        file_name: entry.entry.file_name.is_empty()
            || !matches!(
                entry.entry.namespace,
                FileNameNamespace::Win32 | FileNameNamespace::Win32AndDos
            )
            || entry.hard_link_count > 1,
        data: !entry.entry.is_directory && !entry.have_unnamed_data,
        reparse: entry.is_reparse_point && entry.entry.reparse_tag.is_none(),
    }
}

fn merge_extension_data(entry: &mut RawMftEntry, ext_entry: &RawMftEntry, needs: EnrichmentNeeds) {
    if needs.data
        && (ext_entry.real_size > entry.real_size
            || ext_entry.allocated_size > entry.allocated_size)
    {
        entry.real_size = ext_entry.real_size;
        entry.allocated_size = ext_entry.allocated_size;
        entry.has_unnamed_data = ext_entry.has_unnamed_data;
        entry.is_resident = ext_entry.is_resident;
        entry.data_run_summary = ext_entry.data_run_summary.clone();
    }
    if needs.data {
        entry.is_sparse |= ext_entry.is_sparse;
        entry.is_compressed |= ext_entry.is_compressed;
        entry.is_encrypted |= ext_entry.is_encrypted;
    }
    if needs.reparse {
        entry.is_reparse_point |= ext_entry.is_reparse_point;
        entry.reparse_tag = ext_entry.reparse_tag;
    }
    if needs.data && !ext_entry.alternate_data_streams.is_empty() {
        let mut ads = entry.alternate_data_streams.to_vec();
        ads.extend_from_slice(&ext_entry.alternate_data_streams);
        entry.alternate_data_streams = ads.into_boxed_slice();
    }
}

fn merge_batch_extension_data(
    entry: &mut RawMftBatchScratch,
    ext_entry: &RawMftBatchScratch,
    needs: EnrichmentNeeds,
) {
    if needs.data
        && (ext_entry.entry.real_size > entry.entry.real_size
            || ext_entry.entry.allocated_size > entry.entry.allocated_size)
    {
        entry.entry.real_size = ext_entry.entry.real_size;
        entry.entry.allocated_size = ext_entry.entry.allocated_size;
        entry.have_unnamed_data = ext_entry.have_unnamed_data;
    }
    if needs.reparse {
        entry.is_reparse_point |= ext_entry.is_reparse_point;
        entry.entry.reparse_tag = ext_entry.entry.reparse_tag;
    }
}

/// Merge file-name links from an extension record into a rich raw entry.
fn merge_extension_links(entry: &mut RawMftEntry, ext_entry: &RawMftEntry) {
    if let Some(links) = merged_links(
        LinkView {
            links: &entry.links,
            parent_reference: entry.parent_reference,
            namespace: entry.namespace,
            file_name: entry.file_name.as_os_str(),
        },
        LinkView {
            links: &ext_entry.links,
            parent_reference: ext_entry.parent_reference,
            namespace: ext_entry.namespace,
            file_name: ext_entry.file_name.as_os_str(),
        },
    ) {
        entry.links = links;
    }
}

/// Merge file-name links from an extension record into a lean batch entry.
fn merge_batch_extension_links(entry: &mut RawMftBatchScratch, ext_entry: &RawMftBatchScratch) {
    if let Some(links) = merged_links(
        LinkView {
            links: &entry.entry.links,
            parent_reference: entry.entry.parent_reference,
            namespace: entry.entry.namespace,
            file_name: entry.entry.file_name.as_os_str(),
        },
        LinkView {
            links: &ext_entry.entry.links,
            parent_reference: ext_entry.entry.parent_reference,
            namespace: ext_entry.entry.namespace,
            file_name: ext_entry.entry.file_name.as_os_str(),
        },
    ) {
        entry.entry.links = links;
    }
}

/// Build the merged set of file-name links contributed by a base record and
/// one extension record.
fn merged_links(base: LinkView<'_>, ext: LinkView<'_>) -> Option<Box<[RawMftLink]>> {
    let has_single_ext_link = ext.links.is_empty() && !ext.file_name.is_empty();
    if !has_single_ext_link && ext.links.is_empty() {
        return None;
    }

    let base_link_count = usize::from(base.links.is_empty() && !base.file_name.is_empty());
    let ext_link_count = if has_single_ext_link {
        1
    } else {
        ext.links.len()
    };
    let mut links = Vec::with_capacity(base.links.len() + base_link_count + ext_link_count);
    links.extend_from_slice(base.links);
    if links.is_empty() && !base.file_name.is_empty() {
        links.push(RawMftLink {
            parent_reference: base.parent_reference,
            namespace: base.namespace,
            file_name: base.file_name.to_os_string(),
        });
    }
    if has_single_ext_link {
        links.push(RawMftLink {
            parent_reference: ext.parent_reference,
            namespace: ext.namespace,
            file_name: ext.file_name.to_os_string(),
        });
    } else {
        links.extend_from_slice(ext.links);
    }
    Some(links.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    use crate::raw_mft::ondisk::attribute::AttributeListEntryHeader;

    fn attr_list_entry(type_id: u32, record_number: u64) -> Vec<u8> {
        let header = AttributeListEntryHeader {
            type_id,
            record_length: std::mem::size_of::<AttributeListEntryHeader>() as u16,
            attribute_name_length: 0,
            attribute_name_offset: 0,
            lowest_vcn: 0,
            file_reference: record_number,
            attribute_id: 0,
        };
        let mut bytes = vec![0u8; std::mem::size_of::<AttributeListEntryHeader>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr() as *mut AttributeListEntryHeader, header);
        }
        bytes
    }

    #[test]
    fn collect_extension_records_filters_base_and_duplicates() {
        let base_record = 42;
        let mut data = Vec::new();
        data.extend_from_slice(&attr_list_entry(
            NtfsAttributeType::FileName as u32,
            base_record,
        ));
        data.extend_from_slice(&attr_list_entry(NtfsAttributeType::FileName as u32, 100));
        data.extend_from_slice(&attr_list_entry(NtfsAttributeType::Data as u32, 100));
        data.extend_from_slice(&attr_list_entry(NtfsAttributeType::Data as u32, 101));

        let records = collect_extension_records(&data, base_record, |type_id| {
            type_id == NtfsAttributeType::FileName as u32
        });

        assert_eq!(records, vec![100]);
    }

    #[test]
    fn merged_links_includes_inline_base_and_extension_names() {
        let base_name = OsString::from("base.txt");
        let ext_name = OsString::from("ext.txt");
        let links = merged_links(
            LinkView {
                links: &[],
                parent_reference: Fid::new(5),
                namespace: FileNameNamespace::Win32,
                file_name: base_name.as_os_str(),
            },
            LinkView {
                links: &[],
                parent_reference: Fid::new(9),
                namespace: FileNameNamespace::Win32,
                file_name: ext_name.as_os_str(),
            },
        )
        .expect("extension name should produce merged links");

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].parent_reference, Fid::new(5));
        assert_eq!(links[1].parent_reference, Fid::new(9));
    }
}
