//! Raw `RawMft` construction and one-time discovery of shared `$MFT` state.
//!
//! This module owns the expensive setup path behind [`super::RawMft::new`]:
//! reading the NTFS boot sector, parsing FILE record 0, decoding the
//! `$MFT::$DATA` extent map, and materializing `$MFT::$BITMAP` for later
//! in-memory record filtering.

use std::io::{Read, Seek, SeekFrom};

use log::debug;

use crate::{
    errors::UsnError,
    raw_mft::{
        boot::BootSector,
        init_support::bootstrap_mft_state,
        io::VolumeReader,
        reader::io_err,
    },
    volume::Volume,
};

use super::RawMft;

impl<'a> RawMft<'a> {
    /// Open the volume's `$MFT`, parse the boot sector and record 0, and
    /// build the shared extent map plus `$BITMAP` snapshot used by later scans.
    pub fn new(volume: &'a Volume) -> Result<Self, UsnError> {
        let boot = read_boot_sector(volume)?;

        debug!(
            "raw_mft: cluster_size={} file_record_size={} mft_lcn={} mft_byte_offset={}",
            boot.cluster_size, boot.file_record_size, boot.mft_lcn, boot.mft_byte_offset
        );
        let bootstrap = bootstrap_mft_state(volume, &boot)?;

        Ok(RawMft {
            volume,
            boot,
            extent_map: bootstrap.extent_map,
            bitmap: bootstrap.bitmap,
        })
    }
}

/// Read and parse the NTFS boot sector from the raw volume.
fn read_boot_sector(volume: &Volume) -> Result<BootSector, UsnError> {
    let mut reader = VolumeReader::new(volume.handle, 512)?;
    let mut boot_buf = vec![0u8; 512];
    reader.seek(SeekFrom::Start(0)).map_err(io_err)?;
    reader.read_exact(&mut boot_buf).map_err(io_err)?;
    BootSector::parse(&boot_buf)
}