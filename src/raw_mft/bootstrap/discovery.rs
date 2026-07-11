//! Stream-discovery helpers for raw-MFT constructor bootstrapping.

use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        entry_build::AttributeListInfo,
        io::VolumeReader,
        layout::{
            attribute::{
                NtfsAttribute, NtfsAttributeType, for_each_attr_list_entry_header,
                for_each_attribute,
            },
            boot::BootSector,
            data_run::{DataRun, decode_runs},
            extent::ExtentMap,
            record::{FileRecord, MFT_RECORD_NUMBER},
        },
        reader::{io_err, read_nonresident},
    },
    volume::Volume,
};

/// Shared immutable state discovered while bootstrapping `RawMft`.
pub(super) struct MftBootstrap {
    /// Decoded unnamed `$MFT::$DATA` extent map.
    pub(super) extent_map: Arc<ExtentMap>,
    /// Materialized `$MFT::$BITMAP` contents when available.
    pub(super) bitmap: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamExtent {
    lowest_vcn: u64,
    record_number: u64,
    attribute_id: u16,
    runs: Vec<DataRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct StreamTarget {
    type_id: u32,
    lowest_vcn: u64,
    record_number: u64,
    attribute_id: u16,
}

struct MftStreamRuns {
    data_extents: Vec<StreamExtent>,
    bitmap_extents: Vec<StreamExtent>,
    bitmap_size: u64,
    attribute_list: Option<AttributeListInfo>,
}

/// Read FILE record 0, discover its stream runs, and materialize the shared
/// extent map plus bitmap snapshot used by later scans.
pub(super) fn bootstrap_mft_state(
    volume: &Volume,
    boot: &BootSector,
) -> Result<MftBootstrap, UsnError> {
    let mut reader = VolumeReader::new(volume, boot.bytes_per_sector as u64)?;
    let mut record0 = read_mft_record_zero(&mut reader, boot)?;
    let mut streams = {
        let record =
            FileRecord::parse(MFT_RECORD_NUMBER, Some(boot.mft_byte_offset), &mut record0)?;
        discover_mft_stream_runs(&record)
    };
    load_extension_stream_extents(&mut reader, boot, &mut streams)?;

    if streams.data_extents.is_empty() {
        return Err(UsnError::MftAttributeMissing("$MFT $DATA"));
    }
    let data_runs = assemble_stream_runs(&streams.data_extents)?;
    let extent_map = Arc::new(ExtentMap::from_runs(
        &data_runs,
        boot.cluster_size,
        boot.file_record_size,
    ));
    let bitmap = load_mft_bitmap(
        &mut reader,
        boot.cluster_size,
        (!streams.bitmap_extents.is_empty())
            .then(|| assemble_stream_runs(&streams.bitmap_extents))
            .transpose()?,
        streams.bitmap_size,
    )?;

    Ok(MftBootstrap { extent_map, bitmap })
}

/// Read FILE record 0 from the raw volume.
fn read_mft_record_zero(reader: &mut VolumeReader, boot: &BootSector) -> Result<Vec<u8>, UsnError> {
    let mut record0 = vec![0u8; boot.file_record_size as usize];
    reader
        .seek(SeekFrom::Start(boot.mft_byte_offset))
        .map_err(io_err)?;
    reader.read_exact(&mut record0).map_err(io_err)?;
    Ok(record0)
}

/// Walk FILE record 0 and collect the decoded runlists for `$MFT::$DATA`
/// and `$MFT::$BITMAP`.
fn discover_mft_stream_runs(record: &FileRecord<'_>) -> MftStreamRuns {
    let mut streams = MftStreamRuns {
        data_extents: Vec::new(),
        bitmap_extents: Vec::new(),
        bitmap_size: 0,
        attribute_list: None,
    };
    capture_stream_attributes(record, &mut streams, true);
    streams
}

/// Capture `$MFT::$DATA` and `$MFT::$BITMAP` extents from one fixed-up record.
fn capture_stream_attributes(
    record: &FileRecord<'_>,
    streams: &mut MftStreamRuns,
    capture_attribute_list: bool,
) {
    let (attrs_off, used) = record.attrs_range();
    for_each_attribute(record.data, attrs_off, used, |attr| {
        let type_id = attr.type_id();
        let unnamed = !attr.has_name();

        if type_id == NtfsAttributeType::Data as u32 && unnamed && attr.is_non_resident() {
            if let Some(extent) = decode_nonresident_extent(attr, record.number, "$MFT $DATA") {
                insert_extent(&mut streams.data_extents, extent);
            }
        } else if type_id == NtfsAttributeType::Bitmap as u32 && unnamed && attr.is_non_resident() {
            if let Some(header) = attr.nonresident_header() {
                streams.bitmap_size = streams.bitmap_size.max(header.data_size);
            }
            if let Some(extent) = decode_nonresident_extent(attr, record.number, "$MFT $BITMAP") {
                insert_extent(&mut streams.bitmap_extents, extent);
            }
        } else if capture_attribute_list
            && type_id == NtfsAttributeType::AttributeList as u32
            && streams.attribute_list.is_none()
        {
            streams.attribute_list = capture_attribute_list_info(attr);
        }
    });
}

fn insert_extent(extents: &mut Vec<StreamExtent>, extent: StreamExtent) {
    if !extents.iter().any(|existing| {
        existing.lowest_vcn == extent.lowest_vcn
            && existing.record_number == extent.record_number
            && existing.attribute_id == extent.attribute_id
    }) {
        extents.push(extent);
    }
}

fn capture_attribute_list_info(attr: &NtfsAttribute<'_>) -> Option<AttributeListInfo> {
    if attr.is_non_resident() {
        let header = attr.nonresident_header()?;
        let start = header.data_runs_offset as usize;
        Some(AttributeListInfo::NonResident {
            runs_data: attr.data().get(start..)?.to_vec(),
            data_size: header.data_size,
        })
    } else {
        attr.resident_value()
            .map(|value| AttributeListInfo::Resident(value.to_vec()))
    }
}

/// Load stream extents referenced by record 0's `$ATTRIBUTE_LIST`.
///
/// This is iterative because a newly discovered `$DATA` extent can make a
/// later extension record addressable through the growing partial extent map.
fn load_extension_stream_extents(
    reader: &mut VolumeReader,
    boot: &BootSector,
    streams: &mut MftStreamRuns,
) -> Result<(), UsnError> {
    let Some(attribute_list) = streams.attribute_list.take() else {
        return Ok(());
    };
    let data = materialize_attribute_list(reader, boot.cluster_size, attribute_list)?;
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for_each_attr_list_entry_header(&data, |header| {
        let type_id = header.type_id;
        let is_stream = (type_id == NtfsAttributeType::Data as u32
            || type_id == NtfsAttributeType::Bitmap as u32)
            && header.attribute_name_length == 0;
        if !is_stream || header.lowest_vcn < 0 {
            return;
        }
        let target = StreamTarget {
            type_id,
            lowest_vcn: header.lowest_vcn as u64,
            record_number: header.file_reference & 0x0000_FFFF_FFFF_FFFF,
            attribute_id: header.attribute_id,
        };
        if seen.insert(target) {
            targets.push(target);
        }
    });

    loop {
        let unresolved: Vec<_> = targets
            .iter()
            .copied()
            .filter(|target| !target_is_loaded(streams, *target))
            .collect();
        if unresolved.is_empty() {
            return Ok(());
        }

        let prefix_runs = assemble_contiguous_prefix(&streams.data_extents)?;
        if prefix_runs.is_empty() {
            return Err(UsnError::MftAttributeMissing("$MFT $DATA VCN 0 extent"));
        }
        let partial_map =
            ExtentMap::from_runs(&prefix_runs, boot.cluster_size, boot.file_record_size);
        let before = streams.data_extents.len() + streams.bitmap_extents.len();
        let mut loaded_records = HashSet::new();

        for target in unresolved {
            if target.record_number == MFT_RECORD_NUMBER
                || !loaded_records.insert(target.record_number)
            {
                continue;
            }
            let offset = match partial_map.record_offset(target.record_number) {
                Ok(Some(offset)) => offset,
                Ok(None) | Err(_) => continue,
            };
            let buf = reader
                .borrow_at(offset, boot.file_record_size as usize)
                .map_err(io_err)?;
            if !FileRecord::is_valid(buf) {
                continue;
            }
            let record = FileRecord::parse(target.record_number, Some(offset), buf)?;
            capture_stream_attributes(&record, streams, false);
        }

        let after = streams.data_extents.len() + streams.bitmap_extents.len();
        if after == before {
            return Err(UsnError::MftAttributeMissing(
                "$MFT stream extension referenced by $ATTRIBUTE_LIST",
            ));
        }
    }
}

fn target_is_loaded(streams: &MftStreamRuns, target: StreamTarget) -> bool {
    let extents = if target.type_id == NtfsAttributeType::Data as u32 {
        &streams.data_extents
    } else {
        &streams.bitmap_extents
    };
    extents.iter().any(|extent| {
        extent.lowest_vcn == target.lowest_vcn
            && extent.record_number == target.record_number
            && extent.attribute_id == target.attribute_id
    })
}

fn materialize_attribute_list(
    reader: &mut VolumeReader,
    cluster_size: u64,
    attribute_list: AttributeListInfo,
) -> Result<Vec<u8>, UsnError> {
    match attribute_list {
        AttributeListInfo::Resident(data) => Ok(data),
        AttributeListInfo::NonResident {
            runs_data,
            data_size,
        } => {
            let (runs, _) = decode_runs(&runs_data)?;
            read_nonresident(reader, &runs, cluster_size, data_size)
        }
    }
}

/// Assemble extents in lowest-VCN order. Mapping pairs in each attribute
/// extent are relative to that extent's `lowest_vcn`, not implicitly VCN zero.
fn assemble_stream_runs(extents: &[StreamExtent]) -> Result<Vec<DataRun>, UsnError> {
    let (runs, complete) = assemble_runs(extents, false)?;
    if !complete {
        return Err(UsnError::InvalidDataRun(
            "non-resident stream extents contain a VCN gap or overlap",
        ));
    }
    Ok(runs)
}

fn assemble_contiguous_prefix(extents: &[StreamExtent]) -> Result<Vec<DataRun>, UsnError> {
    assemble_runs(extents, true).map(|(runs, _)| runs)
}

fn assemble_runs(
    extents: &[StreamExtent],
    stop_at_gap: bool,
) -> Result<(Vec<DataRun>, bool), UsnError> {
    let mut ordered: Vec<_> = extents.iter().collect();
    ordered.sort_by_key(|extent| extent.lowest_vcn);
    let mut expected_vcn = 0u64;
    let mut runs = Vec::new();

    for extent in ordered {
        if extent.lowest_vcn > expected_vcn && stop_at_gap {
            return Ok((runs, false));
        }
        if extent.lowest_vcn < expected_vcn {
            return Err(UsnError::InvalidDataRun(
                "non-resident stream extents overlap in VCN space",
            ));
        }
        if extent.lowest_vcn > expected_vcn {
            return Ok((runs, false));
        }
        let clusters = extent.runs.iter().try_fold(0u64, |total, run| {
            let count = match run {
                DataRun::Data { clusters, .. } | DataRun::Sparse { clusters } => *clusters,
            };
            total
                .checked_add(count)
                .ok_or(UsnError::InvalidDataRun("stream VCN length overflow"))
        })?;
        expected_vcn = expected_vcn
            .checked_add(clusters)
            .ok_or(UsnError::InvalidDataRun("stream VCN end overflow"))?;
        runs.extend_from_slice(&extent.runs);
    }
    Ok((runs, true))
}

fn decode_nonresident_extent(
    attr: &NtfsAttribute<'_>,
    record_number: u64,
    label: &'static str,
) -> Option<StreamExtent> {
    let header = attr.nonresident_header()?;
    if header.lowest_vcn < 0 {
        return None;
    }
    decode_nonresident_runs(attr, label).map(|runs| StreamExtent {
        lowest_vcn: header.lowest_vcn as u64,
        record_number,
        attribute_id: attr.header.id,
        runs,
    })
}

/// Load the `$MFT::$BITMAP` stream when it exists.
fn load_mft_bitmap(
    reader: &mut VolumeReader,
    cluster_size: u64,
    bitmap_runs: Option<Vec<DataRun>>,
    bitmap_size: u64,
) -> Result<Arc<[u8]>, UsnError> {
    if let Some(bitmap_runs) = bitmap_runs {
        Ok(read_nonresident(reader, &bitmap_runs, cluster_size, bitmap_size)?.into())
    } else {
        warn!("raw_mft: no $MFT $BITMAP; unused-record filtering will be unavailable");
        Ok(Vec::new().into())
    }
}

/// Decode the runlist payload of a non-resident attribute from FILE record 0.
fn decode_nonresident_runs(attr: &NtfsAttribute<'_>, label: &'static str) -> Option<Vec<DataRun>> {
    let header = attr.nonresident_header()?;
    let runs_offset = header.data_runs_offset as usize;
    let attr_data = attr.data();
    if runs_offset > attr_data.len() {
        return None;
    }

    match decode_runs(&attr_data[runs_offset..]) {
        Ok((runs, _)) => Some(runs),
        Err(error) => {
            warn!("{label} decode_runs failed: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_mft::layout::attribute::{
        NtfsAttributeHeader, NtfsAttributeType, NtfsNonResidentAttributeHeader,
    };
    use std::mem::size_of;
    use zerocopy::IntoBytes;

    /// Build a non-resident `$DATA` attribute whose runlist starts at
    /// `data_runs_offset` and is followed by `runlist` bytes.
    fn build_nonresident_attr(data_runs_offset: u16, runlist: &[u8]) -> Vec<u8> {
        let header_size = size_of::<NtfsNonResidentAttributeHeader>();
        let total = header_size + runlist.len();
        let header = NtfsNonResidentAttributeHeader {
            attribute_header: NtfsAttributeHeader {
                type_id: NtfsAttributeType::Data as u32,
                length: total as u32,
                is_non_resident: 1,
                name_length: 0,
                name_offset: 0,
                flags: 0,
                id: 0,
            },
            lowest_vcn: 0,
            highest_vcn: 0,
            data_runs_offset,
            compression_unit_exponent: 0,
            _reserved: [0; 5],
            allocated_size: 0,
            data_size: 0,
            initialized_size: 0,
        };
        let mut buf = vec![0u8; total];
        buf[..header_size].copy_from_slice(header.as_bytes());
        buf[header_size..].copy_from_slice(runlist);
        buf
    }

    #[test]
    fn decodes_valid_nonresident_runlist() {
        let header_size = size_of::<NtfsNonResidentAttributeHeader>();
        // Single run: 0x21 length=5, offset=0x0234 -> LCN 564.
        let buf = build_nonresident_attr(header_size as u16, &[0x21, 0x05, 0x34, 0x02, 0x00]);
        let attr = NtfsAttribute::new(&buf).expect("attr");
        let runs = decode_nonresident_runs(&attr, "$TEST").expect("runs");
        assert_eq!(
            runs,
            vec![DataRun::Data {
                lcn: 564,
                clusters: 5
            }]
        );
    }

    #[test]
    fn returns_none_when_runs_offset_past_attribute() {
        let header_size = size_of::<NtfsNonResidentAttributeHeader>();
        let runlist = [0x21u8, 0x05, 0x34, 0x02, 0x00];
        let total = header_size + runlist.len();
        // data_runs_offset points beyond the attribute's own bytes.
        let buf = build_nonresident_attr((total + 10) as u16, &runlist);
        let attr = NtfsAttribute::new(&buf).expect("attr");
        assert!(decode_nonresident_runs(&attr, "$TEST").is_none());
    }

    #[test]
    fn returns_none_for_undecodable_runlist() {
        let header_size = size_of::<NtfsNonResidentAttributeHeader>();
        // Truncated run: header byte promises offset bytes that are absent.
        let buf = build_nonresident_attr(header_size as u16, &[0x21, 0x05]);
        let attr = NtfsAttribute::new(&buf).expect("attr");
        assert!(decode_nonresident_runs(&attr, "$TEST").is_none());
    }

    #[test]
    fn assembles_stream_extents_in_lowest_vcn_order() {
        let extents = vec![
            StreamExtent {
                lowest_vcn: 3,
                record_number: 11,
                attribute_id: 2,
                runs: vec![DataRun::Data {
                    lcn: 200,
                    clusters: 2,
                }],
            },
            StreamExtent {
                lowest_vcn: 0,
                record_number: 0,
                attribute_id: 1,
                runs: vec![DataRun::Data {
                    lcn: 100,
                    clusters: 3,
                }],
            },
        ];

        assert_eq!(
            assemble_stream_runs(&extents).expect("contiguous extents"),
            vec![
                DataRun::Data {
                    lcn: 100,
                    clusters: 3,
                },
                DataRun::Data {
                    lcn: 200,
                    clusters: 2,
                },
            ]
        );
    }

    #[test]
    fn rejects_stream_extent_vcn_gaps() {
        let extents = vec![
            StreamExtent {
                lowest_vcn: 0,
                record_number: 0,
                attribute_id: 1,
                runs: vec![DataRun::Data {
                    lcn: 100,
                    clusters: 2,
                }],
            },
            StreamExtent {
                lowest_vcn: 3,
                record_number: 11,
                attribute_id: 2,
                runs: vec![DataRun::Data {
                    lcn: 200,
                    clusters: 1,
                }],
            },
        ];

        assert!(assemble_stream_runs(&extents).is_err());
        assert_eq!(
            assemble_contiguous_prefix(&extents).expect("usable prefix"),
            vec![DataRun::Data {
                lcn: 100,
                clusters: 2,
            }]
        );
    }

    #[test]
    fn rejects_stream_extent_vcn_overlaps_even_for_prefix_assembly() {
        let extents = vec![
            StreamExtent {
                lowest_vcn: 0,
                record_number: 0,
                attribute_id: 1,
                runs: vec![DataRun::Data {
                    lcn: 100,
                    clusters: 3,
                }],
            },
            StreamExtent {
                lowest_vcn: 2,
                record_number: 11,
                attribute_id: 2,
                runs: vec![DataRun::Data {
                    lcn: 200,
                    clusters: 1,
                }],
            },
        ];

        assert!(assemble_stream_runs(&extents).is_err());
        assert!(assemble_contiguous_prefix(&extents).is_err());
    }

    #[test]
    fn target_loadedness_includes_source_record_number() {
        let streams = MftStreamRuns {
            data_extents: vec![StreamExtent {
                lowest_vcn: 4,
                record_number: 10,
                attribute_id: 2,
                runs: vec![DataRun::Data {
                    lcn: 100,
                    clusters: 1,
                }],
            }],
            bitmap_extents: Vec::new(),
            bitmap_size: 0,
            attribute_list: None,
        };

        assert!(target_is_loaded(
            &streams,
            StreamTarget {
                type_id: NtfsAttributeType::Data as u32,
                lowest_vcn: 4,
                record_number: 10,
                attribute_id: 2,
            }
        ));
        assert!(!target_is_loaded(
            &streams,
            StreamTarget {
                type_id: NtfsAttributeType::Data as u32,
                lowest_vcn: 4,
                record_number: 11,
                attribute_id: 2,
            }
        ));
    }
}
