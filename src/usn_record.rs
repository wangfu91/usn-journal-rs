use crate::errors::UsnError;
use std::{
    ffi::OsString,
    mem::{MaybeUninit, offset_of, size_of},
    os::windows::ffi::OsStringExt,
};
use windows::Win32::System::Ioctl::USN_RECORD_V2;

const USN_RECORD_V2_HEADER_LEN: usize = offset_of!(USN_RECORD_V2, FileName);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct UsnRecordV2Header {
    pub(crate) record_length: u32,
    pub(crate) major_version: u16,
    pub(crate) _minor_version: u16,
    pub(crate) file_reference_number: u64,
    pub(crate) parent_file_reference_number: u64,
    pub(crate) usn: i64,
    pub(crate) timestamp: i64,
    pub(crate) reason: u32,
    pub(crate) source_info: u32,
    pub(crate) _security_id: u32,
    pub(crate) file_attributes: u32,
    pub(crate) file_name_length: u16,
    pub(crate) file_name_offset: u16,
}

pub(crate) fn read_unaligned_from<T: Copy>(buffer: &[u8], offset: usize) -> Option<T> {
    let bytes = buffer.get(offset..offset.checked_add(size_of::<T>())?)?;
    Some(unsafe { (bytes.as_ptr() as *const T).read_unaligned() })
}

pub(crate) fn parse_usn_record_v2_header(
    buffer: &[u8],
    offset: u32,
    bytes_read: u32,
    context: &str,
) -> Result<(UsnRecordV2Header, u32), UsnError> {
    let base = offset as usize;
    let read_end = bytes_read as usize;
    let header_bytes = buffer
        .get(base..base.checked_add(USN_RECORD_V2_HEADER_LEN).unwrap())
        .ok_or_else(|| UsnError::OtherError(format!("{context} missing fixed header")))?;
    let mut header = MaybeUninit::<UsnRecordV2Header>::zeroed();
    unsafe {
        std::ptr::copy_nonoverlapping(
            header_bytes.as_ptr(),
            header.as_mut_ptr().cast::<u8>(),
            USN_RECORD_V2_HEADER_LEN,
        );
    }
    let header = unsafe { header.assume_init() };

    if header.record_length == 0 {
        return Err(UsnError::OtherError(format!(
            "{context} contains invalid zero RecordLength"
        )));
    }

    let record_end = base
        .checked_add(header.record_length as usize)
        .ok_or_else(|| UsnError::OtherError(format!("{context} length overflow")))?;
    if record_end > read_end {
        return Err(UsnError::OtherError(format!(
            "{context} extends past buffer bounds"
        )));
    }

    if header.major_version != 2 {
        return Err(UsnError::OtherError(format!(
            "Unsupported {context} version: {}",
            header.major_version
        )));
    }

    Ok((header, header.record_length))
}

pub(crate) fn parse_usn_record_v2_name(
    buffer: &[u8],
    base: usize,
    header: &UsnRecordV2Header,
    context: &str,
) -> Result<OsString, UsnError> {
    let file_name_len = header.file_name_length as usize;
    if !file_name_len.is_multiple_of(size_of::<u16>()) {
        return Err(UsnError::OtherError(format!(
            "{context} file name length is not UTF-16 aligned"
        )));
    }

    let file_name_offset = header.file_name_offset as usize;
    if file_name_offset < USN_RECORD_V2_HEADER_LEN
        || file_name_offset
            .checked_add(file_name_len)
            .filter(|end| *end <= header.record_length as usize)
            .is_none()
    {
        return Err(UsnError::OtherError(format!(
            "{context} file name range is out of bounds"
        )));
    }

    let name_start = base
        .checked_add(file_name_offset)
        .ok_or_else(|| UsnError::OtherError(format!("{context} file name offset overflow")))?;
    let name_end = name_start
        .checked_add(file_name_len)
        .ok_or_else(|| UsnError::OtherError(format!("{context} file name length overflow")))?;
    let name_bytes = buffer.get(name_start..name_end).ok_or_else(|| {
        UsnError::OtherError(format!("{context} file name range is out of bounds"))
    })?;
    let name_units = name_bytes
        .chunks_exact(size_of::<u16>())
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();

    Ok(OsString::from_wide(&name_units))
}
