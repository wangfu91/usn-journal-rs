//! Low-level parsing helpers for raw USN journal and MFT buffers.
//!
//! This module validates the raw Windows FSCTL output, exposes a borrowed
//! view over `USN_RECORD_V2` / `USN_RECORD_V3`, and converts the records into
//! the smaller owned types used by the rest of the crate.

use crate::{Fid, Usn, UsnError, UsnResult, unaligned::read_unaligned_at};
use std::mem::size_of;
use windows::Win32::Storage::FileSystem::FILE_ID_128;
use windows::Win32::System::Ioctl::{USN_RECORD_COMMON_HEADER, USN_RECORD_V2, USN_RECORD_V3};

/// Borrowed view over a raw USN record.
///
/// The enum hides the version-specific Windows layouts so callers can read
/// common fields without duplicating the parser logic.
#[derive(Copy, Clone, Debug)]
pub(crate) enum UsnRecordView<'a> {
    /// Borrowed `USN_RECORD_V2` view.
    V2(&'a USN_RECORD_V2),
    /// Borrowed `USN_RECORD_V3` view.
    V3(&'a USN_RECORD_V3),
}

impl<'a> UsnRecordView<'a> {
    /// Raw Update Sequence Number from the record.
    #[inline]
    pub(crate) const fn usn(self) -> i64 {
        match self {
            Self::V2(record) => record.Usn,
            Self::V3(record) => record.Usn,
        }
    }

    /// Raw FILETIME timestamp from the record.
    #[inline]
    pub(crate) const fn timestamp(self) -> i64 {
        match self {
            Self::V2(record) => record.TimeStamp,
            Self::V3(record) => record.TimeStamp,
        }
    }

    /// Raw USN reason bitmask from the record.
    #[inline]
    pub(crate) const fn reason(self) -> u32 {
        match self {
            Self::V2(record) => record.Reason,
            Self::V3(record) => record.Reason,
        }
    }

    /// Raw source-info bitmask from the record.
    #[inline]
    pub(crate) const fn source_info(self) -> u32 {
        match self {
            Self::V2(record) => record.SourceInfo,
            Self::V3(record) => record.SourceInfo,
        }
    }

    /// Raw file-attribute bitmask from the record.
    #[inline]
    pub(crate) const fn file_attributes(self) -> u32 {
        match self {
            Self::V2(record) => record.FileAttributes,
            Self::V3(record) => record.FileAttributes,
        }
    }

    /// File identifier stored in the record.
    #[inline]
    pub(crate) fn fid(self) -> Fid {
        match self {
            Self::V2(record) => Fid::new(record.FileReferenceNumber),
            Self::V3(record) => Fid::from(file_id_128_to_u128(record.FileReferenceNumber)),
        }
    }

    /// Parent file identifier stored in the record.
    #[inline]
    pub(crate) fn parent_fid(self) -> Fid {
        match self {
            Self::V2(record) => Fid::new(record.ParentFileReferenceNumber),
            Self::V3(record) => Fid::from(file_id_128_to_u128(record.ParentFileReferenceNumber)),
        }
    }

    /// File-name length in bytes.
    #[inline]
    pub(crate) const fn file_name_length(self) -> u16 {
        match self {
            Self::V2(record) => record.FileNameLength,
            Self::V3(record) => record.FileNameLength,
        }
    }

    /// File-name offset in bytes from the start of the record.
    #[inline]
    pub(crate) const fn file_name_offset(self) -> u16 {
        match self {
            Self::V2(record) => record.FileNameOffset,
            Self::V3(record) => record.FileNameOffset,
        }
    }

    /// Pointer to the first UTF-16 code unit of the file name.
    #[inline]
    pub(crate) fn file_name_ptr(self) -> *const u16 {
        match self {
            Self::V2(record) => record.FileName.as_ptr(),
            Self::V3(record) => record.FileName.as_ptr(),
        }
    }

    /// Borrow the UTF-16 file name as a slice of code units.
    #[inline]
    pub(crate) fn file_name_slice(self) -> &'a [u16] {
        let file_name_len = self.file_name_length() as usize / std::mem::size_of::<u16>();
        // SAFETY: Callers only obtain `UsnRecordView` from `find_next_record`,
        // which validates that `FileNameOffset + FileNameLength` stays within
        // the record bounds and that the length is aligned to UTF-16 units.
        unsafe { std::slice::from_raw_parts(self.file_name_ptr(), file_name_len) }
    }
}

/// Convert a Windows `FILE_ID_128` to a native `u128`.
#[inline]
pub(crate) const fn file_id_128_to_u128(file_id: FILE_ID_128) -> u128 {
    u128::from_le_bytes(file_id.Identifier)
}

/// Validate `bytes_read` against `buffer` and convert it to `usize`.
fn checked_bytes_read(buffer: &[u8], bytes_read: u32) -> UsnResult<usize> {
    let bytes_read = bytes_read as usize;
    if bytes_read > buffer.len() {
        return Err(UsnError::InvalidBytesRead {
            bytes_read,
            buffer_len: buffer.len(),
        });
    }
    Ok(bytes_read)
}

/// Read the next USN cursor from the start of an enumeration buffer.
pub(crate) fn read_next_start_usn(buffer: &[u8], bytes_read: u32) -> UsnResult<Usn> {
    let bytes_read = checked_bytes_read(buffer, bytes_read)?;
    let cursor_len = size_of::<Usn>();
    if bytes_read < cursor_len {
        return Err(UsnError::TruncatedRecord {
            offset: 0,
            needed: cursor_len,
            got: bytes_read,
        });
    }

    let Some(raw_value) = read_unaligned_at::<i64>(buffer, 0) else {
        return Err(UsnError::TruncatedRecord {
            offset: 0,
            needed: cursor_len,
            got: bytes_read,
        });
    };
    Ok(Usn::new(i64::from_le(raw_value)))
}

/// Read the next file-ID cursor from the start of an MFT enumeration buffer.
pub(crate) fn read_next_start_fid(buffer: &[u8], bytes_read: u32) -> UsnResult<u64> {
    let bytes_read = checked_bytes_read(buffer, bytes_read)?;
    let cursor_len = size_of::<u64>();
    if bytes_read < cursor_len {
        return Err(UsnError::TruncatedRecord {
            offset: 0,
            needed: cursor_len,
            got: bytes_read,
        });
    }

    let Some(raw_value) = read_unaligned_at::<u64>(buffer, 0) else {
        return Err(UsnError::TruncatedRecord {
            offset: 0,
            needed: cursor_len,
            got: bytes_read,
        });
    };
    Ok(u64::from_le(raw_value))
}

/// Parse the next USN record and advance `offset` past it.
pub(crate) fn find_next_record<'a>(
    buffer: &'a [u8],
    bytes_read: u32,
    offset: &mut u32,
) -> UsnResult<Option<UsnRecordView<'a>>> {
    let bytes_read = checked_bytes_read(buffer, bytes_read)?;
    let offset_usize = *offset as usize;

    if offset_usize >= bytes_read {
        return Ok(None);
    }

    let min_record_len = size_of::<USN_RECORD_COMMON_HEADER>();
    if bytes_read - offset_usize < min_record_len {
        return Err(UsnError::TruncatedRecord {
            offset: offset_usize as u64,
            needed: min_record_len,
            got: bytes_read - offset_usize,
        });
    }

    let Some(header) = read_unaligned_at::<USN_RECORD_COMMON_HEADER>(buffer, offset_usize) else {
        return Err(UsnError::TruncatedRecord {
            offset: offset_usize as u64,
            needed: min_record_len,
            got: bytes_read - offset_usize,
        });
    };

    let record_len = header.RecordLength as usize;
    if record_len < min_record_len {
        return Err(UsnError::InvalidRecordLength {
            offset: offset_usize as u64,
            length: header.RecordLength,
            reason: "record length is smaller than header",
        });
    }
    if record_len > bytes_read - offset_usize {
        return Err(UsnError::TruncatedRecord {
            offset: offset_usize as u64,
            needed: record_len,
            got: bytes_read - offset_usize,
        });
    }

    let record = match header.MajorVersion {
        2 => {
            if record_len < size_of::<USN_RECORD_V2>() {
                return Err(UsnError::InvalidRecordLength {
                    offset: offset_usize as u64,
                    length: header.RecordLength,
                    reason: "record length is smaller than USN_RECORD_V2",
                });
            }
            // SAFETY: `record_len` has been validated against the V2 header size
            // and stays within `buffer`. The FSCTL buffer is 8-byte aligned and
            // USN records are quad-aligned, so reinterpreting the record bytes
            // as `USN_RECORD_V2` is sound for the lifetime of `buffer`.
            let record = unsafe { &*(buffer.as_ptr().add(offset_usize) as *const USN_RECORD_V2) };
            UsnRecordView::V2(record)
        }
        3 => {
            if record_len < size_of::<USN_RECORD_V3>() {
                return Err(UsnError::InvalidRecordLength {
                    offset: offset_usize as u64,
                    length: header.RecordLength,
                    reason: "record length is smaller than USN_RECORD_V3",
                });
            }
            // SAFETY: same argument as the V2 branch above, but for the V3
            // layout requested via `READ_USN_JOURNAL_DATA_V1` /
            // `MFT_ENUM_DATA_V1`.
            let record = unsafe { &*(buffer.as_ptr().add(offset_usize) as *const USN_RECORD_V3) };
            UsnRecordView::V3(record)
        }
        _ => {
            return Err(UsnError::UnsupportedRecordVersion {
                offset: offset_usize as u64,
                major_version: header.MajorVersion,
            });
        }
    };

    let file_name_offset = record.file_name_offset() as usize;
    let file_name_length = record.file_name_length() as usize;
    if !file_name_length.is_multiple_of(size_of::<u16>()) {
        return Err(UsnError::MisalignedRecord {
            offset: offset_usize as u64,
            reason: "file name length is not aligned to UTF-16 units",
        });
    }
    let file_name_end =
        file_name_offset
            .checked_add(file_name_length)
            .ok_or(UsnError::InvalidRecord {
                offset: offset_usize as u64,
                reason: "file name range overflowed",
            })?;
    if file_name_end > record_len {
        return Err(UsnError::InvalidRecord {
            offset: offset_usize as u64,
            reason: "file name range exceeds record length",
        });
    }

    let next_offset = offset_usize
        .checked_add(record_len)
        .ok_or(UsnError::InvalidRecord {
            offset: offset_usize as u64,
            reason: "next record offset overflowed",
        })?;

    *offset = next_offset as u32;
    Ok(Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_header(buf: &mut [u8], record_length: u32, major_version: u16) {
        let header = USN_RECORD_COMMON_HEADER {
            RecordLength: record_length,
            MajorVersion: major_version,
            MinorVersion: 0,
        };
        unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr() as *mut USN_RECORD_COMMON_HEADER, header);
        }
    }

    #[test]
    fn cursor_read_rejects_too_large_bytes_read() {
        let err = read_next_start_fid(&[0; 4], 8).unwrap_err();
        assert!(matches!(
            err,
            UsnError::InvalidBytesRead {
                bytes_read: 8,
                buffer_len: 4
            }
        ));
    }

    #[test]
    fn cursor_read_reports_truncated_cursor() {
        let err = read_next_start_usn(&[0; 4], 4).unwrap_err();
        assert!(matches!(
            err,
            UsnError::TruncatedRecord {
                offset: 0,
                needed: 8,
                got: 4
            }
        ));
    }

    #[test]
    fn record_parse_reports_truncated_header() {
        let mut offset = 0;
        let err = find_next_record(&[0; 2], 2, &mut offset).unwrap_err();
        assert!(matches!(
            err,
            UsnError::TruncatedRecord {
                offset: 0,
                needed,
                got: 2
            } if needed == size_of::<USN_RECORD_COMMON_HEADER>()
        ));
    }

    #[test]
    fn record_parse_reports_invalid_record_length() {
        let header_len = size_of::<USN_RECORD_COMMON_HEADER>();
        let mut buf = vec![0u8; header_len];
        write_header(&mut buf, (header_len - 1) as u32, 2);

        let mut offset = 0;
        let err = find_next_record(&buf, buf.len() as u32, &mut offset).unwrap_err();
        assert!(matches!(
            err,
            UsnError::InvalidRecordLength {
                offset: 0,
                length,
                reason: "record length is smaller than header"
            } if length == (header_len - 1) as u32
        ));
    }

    #[test]
    fn record_parse_reports_unsupported_version() {
        let header_len = size_of::<USN_RECORD_COMMON_HEADER>();
        let mut buf = vec![0u8; header_len];
        write_header(&mut buf, header_len as u32, 99);

        let mut offset = 0;
        let err = find_next_record(&buf, buf.len() as u32, &mut offset).unwrap_err();
        assert!(matches!(
            err,
            UsnError::UnsupportedRecordVersion {
                offset: 0,
                major_version: 99
            }
        ));
    }
}
