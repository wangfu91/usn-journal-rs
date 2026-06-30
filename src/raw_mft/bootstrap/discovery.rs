//! Stream-discovery helpers for raw-MFT constructor bootstrapping.

use std::{
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

use log::warn;

use crate::{
    errors::UsnError,
    raw_mft::{
        io::VolumeReader,
        layout::{
            attribute::{NtfsAttribute, NtfsAttributeType, for_each_attribute},
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

struct MftStreamRuns {
    data_runs: Option<Vec<DataRun>>,
    bitmap_runs: Option<Vec<DataRun>>,
    bitmap_size: u64,
}

/// Read FILE record 0, discover its stream runs, and materialize the shared
/// extent map plus bitmap snapshot used by later scans.
pub(super) fn bootstrap_mft_state(
    volume: &Volume,
    boot: &BootSector,
) -> Result<MftBootstrap, UsnError> {
    let mut reader = VolumeReader::new(volume.handle, boot.bytes_per_sector as u64)?;
    let mut record0 = read_mft_record_zero(&mut reader, boot)?;
    let streams = {
        let record =
            FileRecord::parse(MFT_RECORD_NUMBER, Some(boot.mft_byte_offset), &mut record0)?;
        discover_mft_stream_runs(&record)
    };

    let data_runs = streams
        .data_runs
        .ok_or(UsnError::MftAttributeMissing("$MFT $DATA"))?;
    let extent_map = Arc::new(ExtentMap::from_runs(
        &data_runs,
        boot.cluster_size,
        boot.file_record_size,
    ));
    let bitmap = load_mft_bitmap(
        &mut reader,
        boot.cluster_size,
        streams.bitmap_runs,
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
    let mut data_runs = None;
    let mut bitmap_runs = None;
    let mut bitmap_size = 0u64;

    let (attrs_off, used) = record.attrs_range();
    for_each_attribute(record.data, attrs_off, used, |attr| {
        let type_id = attr.type_id();
        let unnamed = !attr.has_name();

        if type_id == NtfsAttributeType::Data as u32 && unnamed && attr.is_non_resident() {
            data_runs = decode_nonresident_runs(attr, "$MFT $DATA");
        } else if type_id == NtfsAttributeType::Bitmap as u32 && attr.is_non_resident() {
            if let Some(header) = attr.nonresident_header() {
                bitmap_size = header.data_size;
            }
            bitmap_runs = decode_nonresident_runs(attr, "$MFT $BITMAP");
        }
    });

    MftStreamRuns {
        data_runs,
        bitmap_runs,
        bitmap_size,
    }
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
