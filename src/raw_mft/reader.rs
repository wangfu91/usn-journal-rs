//! Shared low-level read helpers reused by the raw-MFT iterator,
//! single-record access path, and `$ATTRIBUTE_LIST` enrichment code.

use std::io::{Read, Seek, SeekFrom};

use crate::{
    errors::UsnError,
    raw_mft::{
        attr_list::{enrich_from_attr_list, should_enrich_from_attr_list},
        entry_build::{AttributeListInfo, EntryBuildOptions, RawMftBatchScratch, RawMftEntry},
        io::VolumeReader,
        layout::{boot::BootSector, data_run::DataRun, extent::ExtentMap, record::FileRecord},
        options::RawMftScanOptions,
    },
};

use super::RawMft;

impl<'a> RawMft<'a> {
    /// Create the pair of buffered readers used by serial and chunked scans:
    /// one for the main sequential path and one for extension-record lookups.
    pub(super) fn buffered_readers_for_options(
        &self,
        options: &RawMftScanOptions,
    ) -> Result<(VolumeReader, VolumeReader), UsnError> {
        let reader = VolumeReader::with_buffer_bytes(
            self.volume.handle,
            self.boot.bytes_per_sector as u64,
            options.buffers.main.get(),
        )?;
        let attr_reader = VolumeReader::with_buffer_bytes(
            self.volume.handle,
            self.boot.bytes_per_sector as u64,
            options.buffers.attr.get(),
        )?;
        Ok((reader, attr_reader))
    }
}

/// Convert iterator options into the entry-build options consumed by the
/// rich entry builder.
pub(super) fn entry_build_options(options: &RawMftScanOptions) -> EntryBuildOptions {
    EntryBuildOptions {
        collect_alternate_data_streams: options.entry.collect_alternate_data_streams,
        collect_data_run_summary: options.entry.collect_data_run_summary,
        collect_dos_file_name_links: options.entry.collect_dos_file_name_links,
    }
}

/// Read one raw FILE record and return the rich entry plus any captured
/// `$ATTRIBUTE_LIST` payload.
pub(super) fn read_record_raw(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
    build_options: EntryBuildOptions,
) -> Result<Option<(RawMftEntry, Option<AttributeListInfo>)>, UsnError> {
    let offset = match extent_map.record_offset(record_number)? {
        Some(offset) => offset,
        None => return Ok(None),
    };
    let buf = reader
        .borrow_at(offset, boot.file_record_size as usize)
        .map_err(io_err)?;
    if !FileRecord::is_valid(buf) {
        return Ok(None);
    }
    let rec = FileRecord::parse(record_number, Some(offset), buf)?;
    Ok(Some(RawMftEntry::from_record_with_attr_list(
        &rec,
        build_options,
    )))
}

/// Read one raw FILE record and return the lean batch entry plus any
/// captured `$ATTRIBUTE_LIST` payload.
pub(super) fn read_batch_record_raw(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
    collect_dos_file_name_links: bool,
) -> Result<Option<(RawMftBatchScratch, Option<AttributeListInfo>)>, UsnError> {
    let offset = match extent_map.record_offset(record_number)? {
        Some(offset) => offset,
        None => return Ok(None),
    };
    let buf = reader
        .borrow_at(offset, boot.file_record_size as usize)
        .map_err(io_err)?;
    if !FileRecord::is_valid(buf) {
        return Ok(None);
    }
    let rec = FileRecord::parse(record_number, Some(offset), buf)?;
    Ok(Some(RawMftBatchScratch::from_record_with_attr_list(
        &rec,
        collect_dos_file_name_links,
    )))
}

/// Read a single record and apply one level of `$ATTRIBUTE_LIST` enrichment
/// when the base entry needs data from extension records.
pub(super) fn read_record_at(
    reader: &mut VolumeReader,
    boot: &BootSector,
    extent_map: &ExtentMap,
    record_number: u64,
    build_options: EntryBuildOptions,
) -> Result<Option<RawMftEntry>, UsnError> {
    let (mut entry, attr_list) =
        match read_record_raw(reader, boot, extent_map, record_number, build_options)? {
            Some(tuple) => tuple,
            None => return Ok(None),
        };
    if let Some(attr_list) = attr_list
        && should_enrich_from_attr_list(&entry)
    {
        let _ = enrich_from_attr_list(
            &mut entry,
            attr_list,
            record_number,
            reader,
            boot,
            extent_map,
            build_options,
        );
    }
    Ok(Some(entry))
}

/// Materialize the contents of a non-resident attribute into memory by
/// reading its decoded data runs from disk.
pub(super) fn read_nonresident(
    reader: &mut VolumeReader,
    runs: &[DataRun],
    cluster_size: u64,
    data_size: u64,
) -> Result<Vec<u8>, UsnError> {
    let mut out = Vec::with_capacity(data_size as usize);
    let mut remaining = data_size;
    for run in runs {
        if remaining == 0 {
            break;
        }
        match *run {
            DataRun::Data { lcn, clusters } => {
                let bytes = clusters
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run byte length overflow"))?;
                let to_read = bytes.min(remaining);
                let offset = lcn
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run offset overflow"))?;
                let start = out.len();
                out.resize(start + to_read as usize, 0);
                reader.seek(SeekFrom::Start(offset)).map_err(io_err)?;
                reader.read_exact(&mut out[start..]).map_err(io_err)?;
                remaining -= to_read;
            }
            DataRun::Sparse { clusters } => {
                let bytes = clusters
                    .checked_mul(cluster_size)
                    .ok_or(UsnError::InvalidDataRun("run byte length overflow"))?;
                let to_zero = bytes.min(remaining);
                out.resize(out.len() + to_zero as usize, 0);
                remaining -= to_zero;
            }
        }
    }
    if remaining != 0 {
        return Err(UsnError::InvalidDataRun(
            "data runs shorter than requested attribute size",
        ));
    }
    Ok(out)
}

/// Convert a standard I/O error into the crate's public error type.
pub(super) fn io_err(error: std::io::Error) -> UsnError {
    UsnError::Io(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::HANDLE;

    fn sparse_only_reader() -> VolumeReader {
        VolumeReader::new(HANDLE(std::ptr::null_mut()), 512)
            .expect("test reader construction should succeed")
    }

    #[test]
    fn read_nonresident_accepts_exact_sparse_coverage() {
        let mut reader = sparse_only_reader();
        let out = read_nonresident(&mut reader, &[DataRun::Sparse { clusters: 2 }], 4, 8)
            .expect("exact sparse coverage should succeed");
        assert_eq!(out, vec![0; 8]);
    }

    #[test]
    fn read_nonresident_rejects_short_sparse_coverage() {
        let mut reader = sparse_only_reader();
        let error =
            read_nonresident(&mut reader, &[DataRun::Sparse { clusters: 1 }], 4, 8).unwrap_err();
        assert!(matches!(
            error,
            UsnError::InvalidDataRun("data runs shorter than requested attribute size")
        ));
    }
}

