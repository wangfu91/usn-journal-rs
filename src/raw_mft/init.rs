//! Raw `RawMft` construction and one-time discovery of shared `$MFT` state.
//!
//! This module owns the expensive setup path behind [`super::RawMft::new`]:
//! reading the NTFS boot sector, parsing FILE record 0, decoding the
//! `$MFT::$DATA` extent map, and materializing `$MFT::$BITMAP` for later
//! in-memory record filtering.

use std::{
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

use log::{debug, warn};

use crate::{
    errors::UsnError,
    raw_mft::{
        attribute::{NtfsAttribute, NtfsAttributeType, for_each_attribute},
        boot::BootSector,
        data_run::{DataRun, decode_runs},
        extent::ExtentMap,
        io::VolumeReader,
        reader::{io_err, read_nonresident},
        record::{FileRecord, MFT_RECORD_NUMBER},
    },
    volume::Volume,
};

use super::RawMft;

impl<'a> RawMft<'a> {
    /// Open the volume's `$MFT`, parse the boot sector and record 0, and
    /// build the shared extent map plus `$BITMAP` snapshot used by later scans.
    pub fn new(volume: &'a Volume) -> Result<Self, UsnError> {
        let mut reader = VolumeReader::new(volume.handle, 512)?;

        let mut boot_buf = vec![0u8; 512];
        reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
        reader.read_exact(&mut boot_buf).map_err(io_err)?;
        let boot = BootSector::parse(&boot_buf)?;

        let mut reader = VolumeReader::new(volume.handle, boot.bytes_per_sector as u64)?;

        debug!(
            "raw_mft: cluster_size={} file_record_size={} mft_lcn={} mft_byte_offset={}",
            boot.cluster_size, boot.file_record_size, boot.mft_lcn, boot.mft_byte_offset
        );

        let mut record0 = vec![0u8; boot.file_record_size as usize];
        reader
            .seek(SeekFrom::Start(boot.mft_byte_offset))
            .map_err(io_err)?;
        reader.read_exact(&mut record0).map_err(io_err)?;
        let (data_runs, bitmap_runs, bitmap_size) = {
            let record =
                FileRecord::parse(MFT_RECORD_NUMBER, Some(boot.mft_byte_offset), &mut record0)?;
            collect_mft_stream_runs(&record)
        };

        let data_runs = data_runs.ok_or(UsnError::MftAttributeMissing("$MFT $DATA"))?;
        let extent_map = Arc::new(ExtentMap::from_runs(
            &data_runs,
            boot.cluster_size,
            boot.file_record_size,
        ));

        let bitmap: Arc<[u8]> = if let Some(bitmap_runs) = bitmap_runs {
            read_nonresident(&mut reader, &bitmap_runs, boot.cluster_size, bitmap_size)?.into()
        } else {
            warn!("raw_mft: no $MFT $BITMAP; skip_unused will be ignored");
            Vec::new().into()
        };

        Ok(RawMft {
            volume,
            boot,
            extent_map,
            bitmap,
        })
    }
}

/// Walk FILE record 0 and collect the decoded runlists for `$MFT::$DATA`
/// and `$MFT::$BITMAP`.
fn collect_mft_stream_runs(record: &FileRecord<'_>) -> (Option<Vec<DataRun>>, Option<Vec<DataRun>>, u64) {
    let mut data_runs = None;
    let mut bitmap_runs = None;
    let mut bitmap_size = 0u64;

    let (attrs_off, used) = record.attrs_range();
    for_each_attribute(record.data, attrs_off, used, |attr| {
        let type_id = attr.type_id();
        let unnamed = attr.name_slice().is_none();

        if type_id == NtfsAttributeType::Data as u32 && unnamed && attr.is_non_resident() {
            data_runs = decode_nonresident_runs(attr, "$MFT $DATA");
        } else if type_id == NtfsAttributeType::Bitmap as u32 && attr.is_non_resident() {
            if let Some(header) = attr.nonresident_header() {
                bitmap_size = header.data_size;
            }
            bitmap_runs = decode_nonresident_runs(attr, "$MFT $BITMAP");
        }
    });

    (data_runs, bitmap_runs, bitmap_size)
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