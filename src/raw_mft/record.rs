//! FILE record validation and access helpers.
//!
//! An NTFS FILE record begins with a fixed [`FileRecordHeader`] followed
//! by an Update Sequence Array (USA) and then a sequence of attributes.
//! This module validates the header and exposes the byte slice of
//! attributes after a successful USA fixup.

use std::mem::size_of;

use crate::{errors::UsnError, raw_mft::fixup};

/// `FILE` record signature.
pub const FILE_RECORD_SIGNATURE: &[u8; 4] = b"FILE";

/// Record number of the `$MFT` itself.
pub const MFT_RECORD_NUMBER: u64 = 0;
/// Record number of the volume root directory.
pub const ROOT_RECORD: u64 = 5;
/// First record number that can correspond to user-visible files.
pub const FIRST_NORMAL_RECORD: u64 = 24;

/// FILE record header flags.
pub mod flags {
    pub const IN_USE: u16 = 0x0001;
    pub const IS_DIRECTORY: u16 = 0x0002;
}

#[repr(C, packed)]
pub(crate) struct FileRecordHeader {
    pub signature: [u8; 4],
    pub update_sequence_offset: u16,
    pub update_sequence_length: u16,
    pub logfile_sequence_number: u64,
    pub sequence_value: u16,
    pub link_count: u16,
    pub attributes_offset: u16,
    pub flags: u16,
    pub used_size: u32,
    pub allocated_size: u32,
    pub base_reference: u64,
    pub next_attribute_id: u16,
}

/// View into a parsed FILE record.
pub(crate) struct FileRecord<'a> {
    pub data: &'a [u8],
    pub header: &'a FileRecordHeader,
    pub number: u64,
}

impl<'a> FileRecord<'a> {
    /// Validate that `data` starts with a plausible FILE record header.
    pub fn is_valid(data: &[u8]) -> bool {
        if data.len() < size_of::<FileRecordHeader>() {
            return false;
        }
        let h = unsafe { &*(data.as_ptr() as *const FileRecordHeader) };
        if &h.signature != FILE_RECORD_SIGNATURE {
            return false;
        }
        if h.update_sequence_length == 0 {
            return false;
        }
        if (h.used_size as usize) > data.len() {
            return false;
        }
        let usa_end = h.update_sequence_offset as usize
            + (h.update_sequence_length as usize).saturating_mul(2);
        if usa_end > data.len() {
            return false;
        }
        if (h.attributes_offset as u32) >= h.used_size {
            return false;
        }
        true
    }

    /// Apply the USA fixup to `data` in place and return a borrowing view.
    pub fn parse(number: u64, data: &'a mut [u8]) -> Result<FileRecord<'a>, UsnError> {
        if !Self::is_valid(data) {
            return Err(UsnError::InvalidMftRecord {
                number,
                reason: "FILE signature or header invalid",
            });
        }
        let (usa_offset, usa_count) = {
            let h = unsafe { &*(data.as_ptr() as *const FileRecordHeader) };
            (
                h.update_sequence_offset as usize,
                h.update_sequence_length as usize,
            )
        };
        fixup::apply_fixup(number, data, usa_offset, usa_count)?;
        let header = unsafe { &*(data.as_ptr() as *const FileRecordHeader) };
        Ok(FileRecord {
            data,
            header,
            number,
        })
    }

    pub fn is_used(&self) -> bool {
        self.header.flags & flags::IN_USE != 0
    }

    pub fn is_directory(&self) -> bool {
        self.header.flags & flags::IS_DIRECTORY != 0
    }

    pub fn link_count(&self) -> u16 {
        self.header.link_count
    }

    pub fn sequence_value(&self) -> u16 {
        self.header.sequence_value
    }

    pub fn base_reference(&self) -> u64 {
        self.header.base_reference
    }

    pub fn file_reference(&self) -> u64 {
        let seq = self.sequence_value() as u64;
        (seq << 48) | (self.number & 0x0000_FFFF_FFFF_FFFF)
    }

    pub fn attrs_range(&self) -> (usize, usize) {
        let attrs_off = self.header.attributes_offset as usize;
        let used = (self.header.used_size as usize).min(self.data.len());
        (attrs_off, used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_record() -> Vec<u8> {
        // 1024-byte record, 2 sectors. USA: 1 sentinel + 2 sector replacements.
        let mut buf = vec![0u8; 1024];
        let header = FileRecordHeader {
            signature: *FILE_RECORD_SIGNATURE,
            update_sequence_offset: 42,
            update_sequence_length: 3,
            logfile_sequence_number: 0,
            sequence_value: 1,
            link_count: 1,
            attributes_offset: 56,
            flags: flags::IN_USE,
            used_size: 200,
            allocated_size: 1024,
            base_reference: 0,
            next_attribute_id: 0,
        };
        unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr() as *mut FileRecordHeader, header);
        }
        // USA: sentinel 0xAB 0xCD, then real values for 2 sectors
        buf[42] = 0xAB;
        buf[43] = 0xCD;
        buf[44] = 0x11;
        buf[45] = 0x22;
        buf[46] = 0x33;
        buf[47] = 0x44;
        // Sector trailers: must hold sentinel before fixup.
        buf[510] = 0xAB;
        buf[511] = 0xCD;
        buf[1022] = 0xAB;
        buf[1023] = 0xCD;
        buf
    }

    #[test]
    fn validates_minimal_record() {
        let buf = build_minimal_record();
        assert!(FileRecord::is_valid(&buf));
    }

    #[test]
    fn rejects_bad_signature() {
        let mut buf = build_minimal_record();
        buf[0] = b'X';
        assert!(!FileRecord::is_valid(&buf));
    }

    #[test]
    fn rejects_used_size_too_large() {
        let mut buf = build_minimal_record();
        let used = 99999u32.to_le_bytes();
        // used_size at offset 24
        buf[24..28].copy_from_slice(&used);
        assert!(!FileRecord::is_valid(&buf));
    }

    #[test]
    fn parse_applies_fixup() {
        let mut buf = build_minimal_record();
        let rec = FileRecord::parse(123, &mut buf).expect("parse ok");
        assert!(rec.is_used());
        assert_eq!(rec.link_count(), 1);
        let seq = rec.sequence_value() as u64;
        assert_eq!(rec.file_reference(), (seq << 48) | 123);
        // After fixup the sector trailers must contain the replacements.
        drop(rec);
        assert_eq!(buf[510], 0x11);
        assert_eq!(buf[511], 0x22);
        assert_eq!(buf[1022], 0x33);
        assert_eq!(buf[1023], 0x44);
    }

    #[test]
    fn parse_detects_corruption() {
        let mut buf = build_minimal_record();
        buf[510] = 0xFF;
        match FileRecord::parse(7, &mut buf) {
            Err(UsnError::FixupMismatch { number: 7 }) => {}
            Err(other) => panic!("expected FixupMismatch, got {other:?}"),
            Ok(_) => panic!("expected FixupMismatch, got Ok"),
        }
    }
}
