//! One-level `$ATTRIBUTE_LIST` enrichment for raw and batch MFT entries.
//!
//! NTFS may spill important attributes into extension records when the base
//! FILE record runs out of room. This module decides when that extra work is
//! worthwhile, materializes the flat attribute list, loads the referenced
//! extension records, and merges the relevant metadata back into the caller's
//! base entry.

use std::{ffi::OsStr, time::{Duration, Instant}};

use log::{debug, warn};
use rustc_hash::FxHashSet;

use crate::{
    Fid,
    errors::UsnError,
    raw_mft::{
        attr_list_profile,
        entry_build::{
            AdsInfo, AttributeListInfo, EntryBuildOptions, RawMftBatchScratch, RawMftEntry,
            RawMftLink,
        },
        io::VolumeReader,
        layout::{
            attribute::{FileNameNamespace, NtfsAttributeType, for_each_attr_list_entry},
            boot::BootSector,
            extent::ExtentMap,
        },
        options::AttrListBatchMode,
        reader::{read_batch_record_raw, read_nonresident, read_record_raw},
    },
};

/// Counters collected while enriching one base record from its `$ATTRIBUTE_LIST`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AttrListEnrichStats {
    /// Number of unique extension-record references discovered in the list.
    pub(crate) extension_records_referenced: u64,
    /// Number of referenced extension records that were successfully loaded.
    pub(crate) extension_records_loaded: u64,
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

/// Deferred batch-entry enrichment work prepared from one base record's
/// `$ATTRIBUTE_LIST` and completed later by the chunk-parallel path.
#[derive(Debug)]
pub(crate) struct PreparedBatchAttrListEnrichment {
    /// Batch scratch entry awaiting extension-record merges.
    pub(crate) entry: RawMftBatchScratch,
    /// Base record number whose attr-list generated this work item.
    pub(crate) base_record_number: u64,
    /// Physical offset of the base record when known.
    pub(crate) base_record_offset: Option<u64>,
    /// Referenced extension record numbers in original attr-list order.
    pub(crate) ext_records: Vec<u64>,
    /// Loaded extension entries aligned with `ext_records`.
    pub(crate) loaded_extensions: Vec<Option<RawMftBatchScratch>>,
    /// Attr-list batch mode that decides how loaded extensions are merged.
    mode: AttrListBatchMode,
    /// Metadata categories still missing from the base batch scratch entry.
    needs: EnrichmentNeeds,
}

/// Lightweight per-record attr-list hint used by cost-aware scheduling experiments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BatchAttrListHint {
    pub(crate) attr_list_present: bool,
    pub(crate) nonresident_attr_list: bool,
    pub(crate) needs_enrich: bool,
    pub(crate) referenced_extension_records: u16,
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
    let profile_enabled = attr_list_profile::is_enabled();
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
        false,
        None,
        |type_id| needs.wants_type(type_id),
        |reader, ext_num| {
            read_record_raw(reader, boot, extent_map, ext_num, build_options)
                .map(|result| result.map(|(entry, _)| entry))
        },
        |ext_entry| {
            if needs.file_name {
                let link_merge_started = profile_start(profile_enabled);
                merge_extension_links(entry, &ext_entry);
                record_profile_elapsed(link_merge_started, attr_list_profile::record_link_merge_time);
                if needs.data || needs.reparse {
                    let data_merge_started = profile_start(profile_enabled);
                    merge_extension_data(entry, &ext_entry, needs);
                    record_profile_elapsed(data_merge_started, attr_list_profile::record_data_merge_time);
                }
                let score = ext_entry.namespace.score();
                if score > best_score {
                    let replace_started = profile_start(profile_enabled);
                    best_score = score;
                    entry.namespace = ext_entry.namespace;
                    entry.file_name = ext_entry.file_name;
                    entry.parent_reference = ext_entry.parent_reference;
                    entry.fn_created = ext_entry.fn_created;
                    entry.fn_modified = ext_entry.fn_modified;
                    entry.fn_mft_modified = ext_entry.fn_mft_modified;
                    entry.fn_accessed = ext_entry.fn_accessed;
                    record_profile_elapsed(
                        replace_started,
                        attr_list_profile::record_selected_name_replace_time,
                    );
                }
            } else if needs.data || needs.reparse {
                let data_merge_started = profile_start(profile_enabled);
                merge_extension_data(entry, &ext_entry, needs);
                record_profile_elapsed(data_merge_started, attr_list_profile::record_data_merge_time);
            }
        },
    )
}

/// Return `true` when a batch scratch entry should consult its
/// `$ATTRIBUTE_LIST`.
pub(super) fn should_enrich_batch_from_attr_list(entry: &RawMftBatchScratch) -> bool {
    batch_entry_enrichment_needs(entry).any()
}

/// Return `true` when the ingest summary's lean batch scratch entry should
/// consult its `$ATTRIBUTE_LIST`.
pub(super) fn should_enrich_batch_from_attr_list_for_summary(entry: &RawMftBatchScratch) -> bool {
    summary_batch_entry_enrichment_needs(entry).any()
}

/// Estimate whether one base record's `$ATTRIBUTE_LIST` is likely to be expensive enough to matter for scheduling.
pub(crate) fn estimate_batch_attr_list_hint(
    entry: &RawMftBatchScratch,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    boot: &BootSector,
    mode: AttrListBatchMode,
) -> BatchAttrListHint {
    let needs = match mode {
        AttrListBatchMode::Full => batch_entry_enrichment_needs(entry),
        AttrListBatchMode::SummaryOnly => summary_batch_entry_enrichment_needs(entry),
    };
    let nonresident_attr_list = matches!(attr_list, AttributeListInfo::NonResident { .. });
    let referenced_extension_records = if needs.any() {
        materialize_attr_list(attr_list, base_record_number, reader, boot.cluster_size)
            .map(|data| {
                collect_extension_records(&data, base_record_number, |type_id| needs.wants_type(type_id))
                    .len()
            })
            .unwrap_or(0)
    } else {
        0
    };

    BatchAttrListHint {
        attr_list_present: true,
        nonresident_attr_list,
        needs_enrich: needs.any(),
        referenced_extension_records: referenced_extension_records.min(u16::MAX as usize) as u16,
    }
}

/// Prepare deferred batch enrichment by materializing the base record's
/// `$ATTRIBUTE_LIST` and collecting the referenced extension records that may
/// still contribute useful metadata.
pub(crate) fn prepare_batch_attr_list_enrichment(
    entry: RawMftBatchScratch,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    base_record_offset: Option<u64>,
    reader: &mut VolumeReader,
    boot: &BootSector,
    mode: AttrListBatchMode,
) -> PreparedBatchAttrListEnrichment {
    let profile_enabled = attr_list_profile::is_enabled();
    let needs = match mode {
        AttrListBatchMode::Full => batch_entry_enrichment_needs(&entry),
        AttrListBatchMode::SummaryOnly => summary_batch_entry_enrichment_needs(&entry),
    };

    let ext_records = if needs.any() {
        let materialize_started = profile_start(profile_enabled);
        let data = materialize_attr_list(attr_list, base_record_number, reader, boot.cluster_size);
        record_profile_elapsed(
            materialize_started,
            attr_list_profile::record_attr_list_materialize_time,
        );

        let discovery_started = profile_start(profile_enabled);
        let ext_records = data
            .as_deref()
            .map(|data| {
                collect_extension_records(data, base_record_number, |type_id| {
                    needs.wants_type(type_id)
                })
            })
            .unwrap_or_default();
        record_profile_elapsed(
            discovery_started,
            attr_list_profile::record_extension_record_discovery_time,
        );
        ext_records
    } else {
        Vec::new()
    };

    let mut loaded_extensions = Vec::with_capacity(ext_records.len());
    loaded_extensions.resize_with(ext_records.len(), || None);

    PreparedBatchAttrListEnrichment {
        entry,
        base_record_number,
        base_record_offset,
        ext_records,
        loaded_extensions,
        mode,
        needs,
    }
}

/// Merge any loaded extension entries back into a prepared batch scratch entry
/// in the original attr-list order.
pub(crate) fn apply_prepared_batch_attr_list_enrichment(
    prepared: &mut PreparedBatchAttrListEnrichment,
) -> AttrListEnrichStats {
    let profile_enabled = attr_list_profile::is_enabled();
    let mut stats = AttrListEnrichStats {
        extension_records_referenced: prepared.ext_records.len() as u64,
        extension_records_loaded: 0,
    };
    if !prepared.needs.any() {
        return stats;
    }

    let mut best_score = if prepared.entry.entry.file_name.is_empty() {
        -1
    } else {
        prepared.entry.entry.namespace.score()
    };

    for ext_entry in prepared.loaded_extensions.iter().flatten() {
        stats.extension_records_loaded += 1;
        let visit_started = profile_start(profile_enabled);
        match prepared.mode {
            AttrListBatchMode::Full => {
                if prepared.needs.file_name {
                    let link_merge_started = profile_start(profile_enabled);
                    merge_batch_extension_links(&mut prepared.entry, ext_entry);
                    record_profile_elapsed(
                        link_merge_started,
                        attr_list_profile::record_link_merge_time,
                    );
                    if prepared.needs.data || prepared.needs.reparse {
                        let data_merge_started = profile_start(profile_enabled);
                        merge_batch_extension_data(&mut prepared.entry, ext_entry, prepared.needs);
                        record_profile_elapsed(
                            data_merge_started,
                            attr_list_profile::record_data_merge_time,
                        );
                    }
                    let score = ext_entry.entry.namespace.score();
                    if score > best_score {
                        let replace_started = profile_start(profile_enabled);
                        best_score = score;
                        prepared.entry.entry.namespace = ext_entry.entry.namespace;
                        prepared
                            .entry
                            .entry
                            .file_name
                            .clone_from(&ext_entry.entry.file_name);
                        prepared.entry.entry.parent_reference = ext_entry.entry.parent_reference;
                        record_profile_elapsed(
                            replace_started,
                            attr_list_profile::record_selected_name_replace_time,
                        );
                    }
                } else if prepared.needs.data || prepared.needs.reparse {
                    let data_merge_started = profile_start(profile_enabled);
                    merge_batch_extension_data(&mut prepared.entry, ext_entry, prepared.needs);
                    record_profile_elapsed(
                        data_merge_started,
                        attr_list_profile::record_data_merge_time,
                    );
                }
            }
            AttrListBatchMode::SummaryOnly => {
                if prepared.needs.file_name {
                    let link_merge_started = profile_start(profile_enabled);
                    merge_batch_extension_links(&mut prepared.entry, ext_entry);
                    record_profile_elapsed(
                        link_merge_started,
                        attr_list_profile::record_link_merge_time,
                    );
                }
                if prepared.needs.data {
                    let data_merge_started = profile_start(profile_enabled);
                    merge_batch_extension_data(&mut prepared.entry, ext_entry, prepared.needs);
                    record_profile_elapsed(
                        data_merge_started,
                        attr_list_profile::record_data_merge_time,
                    );
                }
            }
        }
        record_profile_elapsed(
            visit_started,
            attr_list_profile::record_extension_record_visit_time,
        );
    }

    stats
}

/// Enrich a batch scratch entry from one level of extension records.
#[allow(dead_code)]
pub(super) fn enrich_batch_from_attr_list(
    entry: &mut RawMftBatchScratch,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    collect_dos_file_name_links: bool,
    sort_extensions_by_offset: bool,
) -> AttrListEnrichStats {
    let profile_enabled = attr_list_profile::is_enabled();
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
        sort_extensions_by_offset,
        Some(extent_map),
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
                let link_merge_started = profile_start(profile_enabled);
                merge_batch_extension_links(entry, &ext_entry);
                record_profile_elapsed(link_merge_started, attr_list_profile::record_link_merge_time);
                if needs.data || needs.reparse {
                    let data_merge_started = profile_start(profile_enabled);
                    merge_batch_extension_data(entry, &ext_entry, needs);
                    record_profile_elapsed(data_merge_started, attr_list_profile::record_data_merge_time);
                }
                let score = ext_entry.entry.namespace.score();
                if score > best_score {
                    let replace_started = profile_start(profile_enabled);
                    best_score = score;
                    entry.entry.namespace = ext_entry.entry.namespace;
                    entry.entry.file_name = ext_entry.entry.file_name;
                    entry.entry.parent_reference = ext_entry.entry.parent_reference;
                    record_profile_elapsed(
                        replace_started,
                        attr_list_profile::record_selected_name_replace_time,
                    );
                }
            } else if needs.data || needs.reparse {
                let data_merge_started = profile_start(profile_enabled);
                merge_batch_extension_data(entry, &ext_entry, needs);
                record_profile_elapsed(data_merge_started, attr_list_profile::record_data_merge_time);
            }
        },
    )
}

/// Enrich a batch scratch entry using only the metadata needed by the ingest
/// summary path.
#[allow(dead_code)]
pub(super) fn enrich_batch_from_attr_list_for_summary(
    entry: &mut RawMftBatchScratch,
    attr_list: AttributeListInfo,
    base_record_number: u64,
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    collect_dos_file_name_links: bool,
    sort_extensions_by_offset: bool,
) -> AttrListEnrichStats {
    let profile_enabled = attr_list_profile::is_enabled();
    let needs = summary_batch_entry_enrichment_needs(entry);
    if !needs.any() {
        return AttrListEnrichStats::default();
    }

    with_loaded_extension_records(
        attr_list,
        base_record_number,
        reader,
        boot.cluster_size,
        sort_extensions_by_offset,
        Some(extent_map),
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
                let link_merge_started = profile_start(profile_enabled);
                merge_batch_extension_links(entry, &ext_entry);
                record_profile_elapsed(link_merge_started, attr_list_profile::record_link_merge_time);
            }
            if needs.data {
                let data_merge_started = profile_start(profile_enabled);
                merge_batch_extension_data(entry, &ext_entry, needs);
                record_profile_elapsed(data_merge_started, attr_list_profile::record_data_merge_time);
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
            let runs = match crate::raw_mft::layout::data_run::decode_runs(&runs_data) {
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
    sort_extensions_by_offset: bool,
    extent_map: Option<&ExtentMap>,
    wants_type: WantsType,
    mut load_extension: LoadExtension,
    mut visit_extension: VisitExtension,
) -> AttrListEnrichStats
where
    WantsType: Fn(u32) -> bool,
    LoadExtension: FnMut(&mut VolumeReader, u64) -> Result<Option<T>, UsnError>,
    VisitExtension: FnMut(T),
{
    let profile_enabled = attr_list_profile::is_enabled();
    let mut stats = AttrListEnrichStats::default();
    let materialize_started = profile_start(profile_enabled);
    let data = match materialize_attr_list(attr_list, base_record_number, reader, cluster_size) {
        Some(data) => data,
        None => return stats,
    };
    record_profile_elapsed(
        materialize_started,
        attr_list_profile::record_attr_list_materialize_time,
    );

    let discovery_started = profile_start(profile_enabled);
    let ext_records = collect_extension_records(&data, base_record_number, wants_type);
    let load_order =
        extension_record_load_order(&ext_records, extent_map, sort_extensions_by_offset);
    record_profile_elapsed(
        discovery_started,
        attr_list_profile::record_extension_record_discovery_time,
    );
    stats.extension_records_referenced = ext_records.len() as u64;

    let mut loaded_entries = Vec::with_capacity(ext_records.len());
    loaded_entries.resize_with(ext_records.len(), || None);

    for (record_index, ext_num) in load_order {
        let load_started = profile_start(profile_enabled);
        let _extension_scope = attr_list_profile::enter_extension_load_scope();
        let load_result = load_extension(reader, ext_num);
        record_profile_elapsed(
            load_started,
            attr_list_profile::record_extension_record_load_attempt,
        );
        let ext_entry = match load_result {
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
        loaded_entries[record_index] = Some(ext_entry);
    }

    for ext_entry in loaded_entries.into_iter().flatten() {
        let visit_started = profile_start(profile_enabled);
        visit_extension(ext_entry);
        record_profile_elapsed(
            visit_started,
            attr_list_profile::record_extension_record_visit_time,
        );
    }

    stats
}

fn profile_start(enabled: bool) -> Option<Instant> {
    enabled.then(Instant::now)
}

fn record_profile_elapsed<F>(started: Option<Instant>, record: F)
where
    F: FnOnce(Duration),
{
    if let Some(started) = started {
        record(started.elapsed());
    }
}

fn extension_record_load_order(
    ext_records: &[u64],
    extent_map: Option<&ExtentMap>,
    sort_extensions_by_offset: bool,
) -> Vec<(usize, u64)> {
    let mut load_order = ext_records.iter().copied().enumerate().collect::<Vec<_>>();
    if sort_extensions_by_offset {
        if let Some(extent_map) = extent_map {
            load_order.sort_unstable_by_key(|(_, record_number)| {
                match extent_map.record_offset(*record_number) {
                    Ok(Some(offset)) => offset,
                    Ok(None) => u64::MAX - 1,
                    Err(_) => u64::MAX,
                }
            });
        }
    }
    load_order
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

/// Compute the metadata categories needed by the ingest summary path.
fn summary_batch_entry_enrichment_needs(entry: &RawMftBatchScratch) -> EnrichmentNeeds {
    EnrichmentNeeds {
        file_name: entry.entry.file_name.is_empty() || entry.hard_link_count > 1,
        data: !entry.entry.is_directory && !entry.have_unnamed_data,
        reparse: false,
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
        let ads = std::mem::take(&mut entry.alternate_data_streams).into_vec();
        entry.alternate_data_streams = merge_ads_lists(ads, &ext_entry.alternate_data_streams);
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

fn merge_ads_lists(mut ads: Vec<AdsInfo>, ext_ads: &[AdsInfo]) -> Box<[AdsInfo]> {
    ads.reserve(ext_ads.len());
    ads.extend_from_slice(ext_ads);
    ads.into_boxed_slice()
}

/// Merge file-name links from an extension record into a rich raw entry.
fn merge_extension_links(entry: &mut RawMftEntry, ext_entry: &RawMftEntry) {
    let ext = LinkView {
        links: &ext_entry.links,
        parent_reference: ext_entry.parent_reference,
        namespace: ext_entry.namespace,
        file_name: ext_entry.file_name.as_os_str(),
    };
    if !link_view_has_contribution(ext) {
        return;
    }

    let base = LinkView {
        links: &[],
        parent_reference: entry.parent_reference,
        namespace: entry.namespace,
        file_name: entry.file_name.as_os_str(),
    };
    let links = std::mem::take(&mut entry.links).into_vec();
    entry.links = merge_link_views(links, base, ext);
}

/// Merge file-name links from an extension record into a lean batch entry.
fn merge_batch_extension_links(entry: &mut RawMftBatchScratch, ext_entry: &RawMftBatchScratch) {
    let ext = LinkView {
        links: &ext_entry.entry.links,
        parent_reference: ext_entry.entry.parent_reference,
        namespace: ext_entry.entry.namespace,
        file_name: ext_entry.entry.file_name.as_os_str(),
    };
    if !link_view_has_contribution(ext) {
        return;
    }

    let base = LinkView {
        links: &[],
        parent_reference: entry.entry.parent_reference,
        namespace: entry.entry.namespace,
        file_name: entry.entry.file_name.as_os_str(),
    };
    let links = std::mem::take(&mut entry.entry.links).into_vec();
    entry.entry.links = merge_link_views(links, base, ext);
}

/// Build the merged set of file-name links contributed by a base record and
/// one extension record.
#[cfg(test)]
fn merged_links(base: LinkView<'_>, ext: LinkView<'_>) -> Option<Box<[RawMftLink]>> {
    if !link_view_has_contribution(ext) {
        return None;
    }

    Some(merge_link_views(Vec::new(), base, ext))
}

fn link_view_has_contribution(view: LinkView<'_>) -> bool {
    !view.links.is_empty() || !view.file_name.is_empty()
}

fn merge_link_views(
    mut links: Vec<RawMftLink>,
    base: LinkView<'_>,
    ext: LinkView<'_>,
) -> Box<[RawMftLink]> {
    let has_inline_base_link = links.is_empty() && !base.file_name.is_empty();
    let has_single_ext_link = ext.links.is_empty() && !ext.file_name.is_empty();
    let ext_link_count = if has_single_ext_link {
        1
    } else {
        ext.links.len()
    };
    if links.is_empty() {
        links.reserve(base.links.len() + usize::from(has_inline_base_link) + ext_link_count);
        links.extend_from_slice(base.links);
    } else {
        links.reserve(ext_link_count);
    }
    if has_inline_base_link {
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
    attr_list_profile::record_link_merge_shape(
        links.len(),
        usize::from(has_inline_base_link) + usize::from(has_single_ext_link),
        base.links.len() + usize::from(!has_single_ext_link) * ext.links.len(),
    );
    links.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use crate::{FileAttributes, Filetime};
    use zerocopy::IntoBytes;

    use crate::raw_mft::layout::{
        attribute::AttributeListEntryHeader, data_run::DataRun, extent::ExtentMap,
    };

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
        bytes.copy_from_slice(header.as_bytes());
        bytes
    }

    fn ads(name: &str, size: u64) -> AdsInfo {
        AdsInfo {
            name: OsString::from(name),
            real_size: size,
            allocated_size: size,
            is_resident: true,
        }
    }

    fn test_entry(record_number: u64, alternate_data_streams: Vec<AdsInfo>) -> RawMftEntry {
        RawMftEntry {
            record_number,
            sequence_number: 1,
            file_reference: Fid::new(record_number),
            parent_reference: Fid::new(0),
            base_record_reference: 0,
            hard_link_count: 1,
            flags: 0,
            is_used: true,
            is_directory: false,
            is_reparse_point: false,
            reparse_tag: None,
            namespace: FileNameNamespace::Win32,
            file_name: OsString::from("file.txt"),
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
            alternate_data_streams: alternate_data_streams.into_boxed_slice(),
            links: Box::default(),
        }
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

    #[test]
    fn merge_extension_data_appends_ads_across_multiple_extension_merges() {
        let mut base = test_entry(1, vec![ads("base", 1)]);
        let ext_one = test_entry(2, vec![ads("ext1", 2)]);
        let ext_two = test_entry(3, vec![ads("ext2", 3)]);
        let needs = EnrichmentNeeds {
            file_name: false,
            data: true,
            reparse: false,
        };

        merge_extension_data(&mut base, &ext_one, needs);
        merge_extension_data(&mut base, &ext_two, needs);

        let names = base
            .alternate_data_streams
            .iter()
            .map(|ads| ads.name.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["base", "ext1", "ext2"]);
    }

    #[test]
    fn extension_record_load_order_sorts_by_offset_but_keeps_original_indexes() {
        let extent_map = ExtentMap::from_runs(
            &[DataRun::Data {
                lcn: 100,
                clusters: 16,
            }],
            4096,
            1024,
        );
        let ext_records = vec![12, 1, 8, 3];

        let load_order = extension_record_load_order(&ext_records, Some(&extent_map), true);

        assert_eq!(load_order, vec![(1, 1), (3, 3), (2, 8), (0, 12)]);
    }
}
