use crate::{Usn, UsnError, UsnResult};
use std::mem::size_of;
use windows::Win32::System::Ioctl::USN_RECORD_V2;

fn checked_bytes_read(buffer: &[u8], bytes_read: u32) -> UsnResult<usize> {
    let bytes_read = bytes_read as usize;
    if bytes_read > buffer.len() {
        return Err(UsnError::InvalidRecordData(
            "bytes_read exceeds buffer size",
        ));
    }
    Ok(bytes_read)
}

pub(crate) fn read_next_start_usn(buffer: &[u8], bytes_read: u32) -> UsnResult<Usn> {
    let bytes_read = checked_bytes_read(buffer, bytes_read)?;
    let cursor_len = size_of::<Usn>();
    if bytes_read < cursor_len {
        return Err(UsnError::InvalidRecordData(
            "missing next start USN cursor in buffer",
        ));
    }

    let mut raw = [0u8; size_of::<Usn>()];
    raw.copy_from_slice(&buffer[..cursor_len]);
    Ok(Usn::from_le_bytes(raw))
}

pub(crate) fn read_next_start_fid(buffer: &[u8], bytes_read: u32) -> UsnResult<u64> {
    let bytes_read = checked_bytes_read(buffer, bytes_read)?;
    let cursor_len = size_of::<u64>();
    if bytes_read < cursor_len {
        return Err(UsnError::InvalidRecordData(
            "missing next start file ID cursor in buffer",
        ));
    }

    let mut raw = [0u8; size_of::<u64>()];
    raw.copy_from_slice(&buffer[..cursor_len]);
    Ok(u64::from_le_bytes(raw))
}

pub(crate) fn find_next_record<'a>(
    buffer: &'a [u8],
    bytes_read: u32,
    offset: &mut u32,
) -> UsnResult<Option<&'a USN_RECORD_V2>> {
    let bytes_read = checked_bytes_read(buffer, bytes_read)?;
    let offset_usize = *offset as usize;

    if offset_usize >= bytes_read {
        return Ok(None);
    }

    let min_record_len = size_of::<USN_RECORD_V2>();
    if bytes_read - offset_usize < min_record_len {
        return Err(UsnError::InvalidRecordData(
            "insufficient bytes remaining for USN record header",
        ));
    }

    let record = unsafe { &*(buffer.as_ptr().add(offset_usize) as *const USN_RECORD_V2) };

    let record_len = record.RecordLength as usize;
    if record_len < min_record_len {
        return Err(UsnError::InvalidRecordData(
            "record length is smaller than header",
        ));
    }
    if record_len > bytes_read - offset_usize {
        return Err(UsnError::InvalidRecordData(
            "record length exceeds bytes read",
        ));
    }

    let file_name_offset = record.FileNameOffset as usize;
    let file_name_length = record.FileNameLength as usize;
    if !file_name_length.is_multiple_of(size_of::<u16>()) {
        return Err(UsnError::InvalidRecordData(
            "file name length is not aligned to UTF-16 units",
        ));
    }
    let file_name_end = file_name_offset
        .checked_add(file_name_length)
        .ok_or(UsnError::InvalidRecordData("file name range overflowed"))?;
    if file_name_end > record_len {
        return Err(UsnError::InvalidRecordData(
            "file name range exceeds record length",
        ));
    }

    let next_offset = offset_usize
        .checked_add(record_len)
        .ok_or(UsnError::InvalidRecordData("next record offset overflowed"))?;

    *offset = next_offset as u32;
    Ok(Some(record))
}
