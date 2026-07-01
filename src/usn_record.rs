//! Low-level parsing helpers for raw USN journal and MFT buffers.
//!
//! This module validates the raw Windows FSCTL output, exposes a borrowed
//! view over `USN_RECORD_V2` / `USN_RECORD_V3`, and converts the records into
//! the smaller owned types used by the rest of the crate.

use crate::{Fid, Usn, UsnError, UsnResult};
use std::{mem::size_of, ptr};
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

/// Read a `Copy` value from `buffer[offset..]` without requiring alignment.
///
/// The FSCTL output buffers parsed in this module are byte-oriented and may
/// not be naturally aligned for Rust references, so unaligned loads are the
/// correct primitive here after the bounds check succeeds.
#[inline]
fn read_unaligned_at<T: Copy>(buffer: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(size_of::<T>())?;
    if end > buffer.len() {
        return None;
    }

    // SAFETY: The checked range above guarantees the full `T` lies within
    // `buffer`. `read_unaligned` handles any pointer alignment.
    Some(unsafe { ptr::read_unaligned(buffer.as_ptr().add(offset) as *const T) })
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

    /// Build a `USN_RECORD_V2` buffer with the given fields and file name.
    fn build_v2_record(
        usn: i64,
        fid: u64,
        parent: u64,
        reason: u32,
        attributes: u32,
        name: &str,
    ) -> Vec<u8> {
        let name_u16: Vec<u16> = name.encode_utf16().collect();
        let name_len = name_u16.len() * size_of::<u16>();
        let base = size_of::<USN_RECORD_V2>();
        let total = (base + name_len).next_multiple_of(8);
        let name_offset = std::mem::offset_of!(USN_RECORD_V2, FileName);

        let record = USN_RECORD_V2 {
            RecordLength: total as u32,
            MajorVersion: 2,
            MinorVersion: 0,
            FileReferenceNumber: fid,
            ParentFileReferenceNumber: parent,
            Usn: usn,
            TimeStamp: 0,
            Reason: reason,
            SourceInfo: 0,
            SecurityId: 0,
            FileAttributes: attributes,
            FileNameLength: name_len as u16,
            FileNameOffset: name_offset as u16,
            FileName: [0; 1],
        };

        let mut buf = vec![0u8; total];
        // SAFETY: copy the header bytes up to (not including) the FileName
        // placeholder, then splice the real UTF-16 name in at FileNameOffset.
        unsafe {
            ptr::copy_nonoverlapping(
                &record as *const USN_RECORD_V2 as *const u8,
                buf.as_mut_ptr(),
                name_offset,
            );
            ptr::copy_nonoverlapping(
                name_u16.as_ptr() as *const u8,
                buf.as_mut_ptr().add(name_offset),
                name_len,
            );
        }
        buf
    }

    /// Build a `USN_RECORD_V3` buffer carrying 128-bit file IDs.
    fn build_v3_record(usn: i64, fid: u128, parent: u128, name: &str) -> Vec<u8> {
        use windows::Win32::Storage::FileSystem::FILE_ID_128;

        let name_u16: Vec<u16> = name.encode_utf16().collect();
        let name_len = name_u16.len() * size_of::<u16>();
        let base = size_of::<USN_RECORD_V3>();
        let total = (base + name_len).next_multiple_of(8);
        let name_offset = std::mem::offset_of!(USN_RECORD_V3, FileName);

        let record = USN_RECORD_V3 {
            RecordLength: total as u32,
            MajorVersion: 3,
            MinorVersion: 0,
            FileReferenceNumber: FILE_ID_128 {
                Identifier: fid.to_le_bytes(),
            },
            ParentFileReferenceNumber: FILE_ID_128 {
                Identifier: parent.to_le_bytes(),
            },
            Usn: usn,
            TimeStamp: 0,
            Reason: 0,
            SourceInfo: 0,
            SecurityId: 0,
            FileAttributes: 0,
            FileNameLength: name_len as u16,
            FileNameOffset: name_offset as u16,
            FileName: [0; 1],
        };

        let mut buf = vec![0u8; total];
        // SAFETY: same header-then-name splice as the V2 builder.
        unsafe {
            ptr::copy_nonoverlapping(
                &record as *const USN_RECORD_V3 as *const u8,
                buf.as_mut_ptr(),
                name_offset,
            );
            ptr::copy_nonoverlapping(
                name_u16.as_ptr() as *const u8,
                buf.as_mut_ptr().add(name_offset),
                name_len,
            );
        }
        buf
    }

    #[test]
    fn find_next_record_parses_v2_and_advances_offset() {
        let buf = build_v2_record(0x1234, 0x42, 0x7, 0x0000_0100, 0x20, "file.txt");
        let mut offset = 0u32;
        let record = find_next_record(&buf, buf.len() as u32, &mut offset)
            .expect("parse ok")
            .expect("record present");

        assert_eq!(record.usn(), 0x1234);
        assert_eq!(record.fid(), Fid::new(0x42));
        assert_eq!(record.parent_fid(), Fid::new(0x7));
        assert_eq!(record.reason(), 0x0000_0100);
        assert_eq!(record.file_attributes(), 0x20);
        let name: Vec<u16> = "file.txt".encode_utf16().collect();
        assert_eq!(record.file_name_slice(), name.as_slice());
        assert_eq!(offset as usize, buf.len());
    }

    #[test]
    fn find_next_record_iterates_multiple_v2_records() {
        let mut buf = build_v2_record(1, 0x10, 0x5, 0, 0, "a.txt");
        buf.extend(build_v2_record(2, 0x11, 0x5, 0, 0, "bb.txt"));
        let total = buf.len() as u32;

        let mut offset = 0u32;
        let first = find_next_record(&buf, total, &mut offset)
            .unwrap()
            .expect("first record");
        assert_eq!(first.usn(), 1);
        assert_eq!(first.fid(), Fid::new(0x10));

        let second = find_next_record(&buf, total, &mut offset)
            .unwrap()
            .expect("second record");
        assert_eq!(second.usn(), 2);
        assert_eq!(second.fid(), Fid::new(0x11));

        assert_eq!(offset, total);
        assert!(find_next_record(&buf, total, &mut offset).unwrap().is_none());
    }

    #[test]
    fn find_next_record_parses_v3_extended_ids() {
        let fid = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeffu128;
        let parent = 0x1000_0000_0000_0000_0000_0000_0000_0001u128;
        let buf = build_v3_record(0x99, fid, parent, "refs.dat");

        let mut offset = 0u32;
        let record = find_next_record(&buf, buf.len() as u32, &mut offset)
            .unwrap()
            .expect("record present");

        assert_eq!(record.usn(), 0x99);
        assert_eq!(record.fid(), Fid::from(fid));
        assert_eq!(record.parent_fid(), Fid::from(parent));
        assert!(record.fid().is_extended());
    }

    #[test]
    fn find_next_record_rejects_misaligned_file_name_length() {
        let mut buf = build_v2_record(1, 0x10, 0x5, 0, 0, "abc");
        // FileNameLength lives right before FileNameOffset in the header.
        let len_off = std::mem::offset_of!(USN_RECORD_V2, FileNameLength);
        buf[len_off..len_off + 2].copy_from_slice(&3u16.to_le_bytes());

        let mut offset = 0u32;
        let err = find_next_record(&buf, buf.len() as u32, &mut offset).unwrap_err();
        assert!(matches!(err, UsnError::MisalignedRecord { offset: 0, .. }));
    }

    #[test]
    fn find_next_record_rejects_file_name_beyond_record() {
        let mut buf = build_v2_record(1, 0x10, 0x5, 0, 0, "abc");
        // Declare a file-name length that runs past the record end.
        let len_off = std::mem::offset_of!(USN_RECORD_V2, FileNameLength);
        buf[len_off..len_off + 2].copy_from_slice(&1000u16.to_le_bytes());

        let mut offset = 0u32;
        let err = find_next_record(&buf, buf.len() as u32, &mut offset).unwrap_err();
        assert!(matches!(
            err,
            UsnError::InvalidRecord {
                offset: 0,
                reason: "file name range exceeds record length"
            }
        ));
    }

    #[test]
    fn find_next_record_returns_none_when_offset_at_end() {
        let buf = build_v2_record(1, 0x10, 0x5, 0, 0, "a.txt");
        let mut offset = buf.len() as u32;
        assert!(
            find_next_record(&buf, buf.len() as u32, &mut offset)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn read_cursor_helpers_read_leading_values() {
        let mut usn_buf = vec![0u8; 16];
        usn_buf[..8].copy_from_slice(&0x0123_4567i64.to_le_bytes());
        assert_eq!(
            read_next_start_usn(&usn_buf, usn_buf.len() as u32).unwrap(),
            Usn::new(0x0123_4567)
        );

        let mut fid_buf = vec![0u8; 16];
        fid_buf[..8].copy_from_slice(&0xDEAD_BEEFu64.to_le_bytes());
        assert_eq!(
            read_next_start_fid(&fid_buf, fid_buf.len() as u32).unwrap(),
            0xDEAD_BEEF
        );
    }
}
