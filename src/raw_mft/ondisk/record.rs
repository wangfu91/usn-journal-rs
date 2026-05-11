//! FILE record validation and access helpers.
//!
//! An NTFS FILE record begins with a fixed [`FileRecordHeader`] followed
//! by an Update Sequence Array (USA) and then a sequence of attributes.
//! This module validates the header and exposes the byte slice of
//! attributes after a successful USA fixup.

use std::mem::size_of;

use crate::{errors::UsnError, raw_mft::ondisk::usa_fixup};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// `FILE` record signature.
pub const FILE_RECORD_SIGNATURE: &[u8; 4] = b"FILE";

/// Record number of the `$MFT` itself.
pub const MFT_RECORD_NUMBER: u64 = 0;
/// First record number that can correspond to user-visible files.
pub const FIRST_NORMAL_RECORD: u64 = 24;

/// FILE record header flags.
pub mod flags {
    /// Record is currently marked in use.
    pub const IN_USE: u16 = 0x0001;
    /// Record describes a directory.
    pub const IS_DIRECTORY: u16 = 0x0002;
}

/// On-disk FILE record header(42 bytes), as found at the start of every MFT record.
#[repr(C, packed)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
pub(crate) struct FileRecordHeader {
    /// Four-byte `FILE` signature.
    pub signature: [u8; 4],
    /// Byte offset of the update sequence array.
    pub update_sequence_offset: u16,
    /// Number of update sequence array entries.
    pub update_sequence_length: u16,
    /// Log file sequence number.
    pub logfile_sequence_number: u64,
    /// Record sequence number.
    pub sequence_value: u16,
    /// Hard-link count.
    pub link_count: u16,
    /// Byte offset of the first attribute record.
    pub attributes_offset: u16,
    /// Raw FILE-record flags.
    pub flags: u16,
    /// Number of used bytes in the record.
    pub used_size: u32,
    /// Total allocated size of the record.
    pub allocated_size: u32,
    /// Base-record reference for extension records.
    pub base_reference: u64,
    /// Next attribute instance identifier.
    pub next_attribute_id: u16,
}

/// View into a parsed FILE record.
pub(crate) struct FileRecord<'a> {
    /// Fixed-up FILE-record bytes.
    pub data: &'a [u8],
    /// Parsed FILE-record header copied out of the record bytes.
    pub header: FileRecordHeader,
    /// Record number in the `$MFT`.
    pub number: u64,
}

impl<'a> FileRecord<'a> {
    /// Read the fixed-size FILE record header from the start of `data`.
    ///
    /// `zerocopy` performs the size check for us and returns a copied
    /// `FileRecordHeader` value. We keep this as an owned copy on purpose:
    /// the header is only ~42 bytes, even a million records only makes the
    /// copy cost small compared with the full MFT scan, and this avoids
    /// borrowing a packed header across the later in-place USA fixup.
    fn header_from_bytes(data: &[u8]) -> Option<FileRecordHeader> {
        let bytes = data.get(..size_of::<FileRecordHeader>())?;
        FileRecordHeader::read_from_bytes(bytes).ok()
    }

    /// Read and validate a plausible FILE record header from `data`.
    fn validated_header(data: &[u8]) -> Option<FileRecordHeader> {
        let h = Self::header_from_bytes(data)?;
        if &h.signature != FILE_RECORD_SIGNATURE {
            return None;
        }
        if h.update_sequence_length == 0 {
            return None;
        }
        if (h.used_size as usize) > data.len() {
            return None;
        }
        let usa_end = h.update_sequence_offset as usize
            + (h.update_sequence_length as usize).saturating_mul(2);
        if usa_end > data.len() {
            return None;
        }
        if (h.attributes_offset as u32) >= h.used_size {
            return None;
        }
        Some(h)
    }

    /// Validate that `data` starts with a plausible FILE record header.
    pub fn is_valid(data: &[u8]) -> bool {
        Self::validated_header(data).is_some()
    }

    /// Apply the USA fixup to `data` in place and return a borrowing view.
    pub fn parse(
        number: u64,
        volume_offset: Option<u64>,
        data: &'a mut [u8],
    ) -> Result<FileRecord<'a>, UsnError> {
        let header = Self::validated_header(data).ok_or_else(|| {
            UsnError::invalid_mft_record(number, volume_offset, "FILE signature or header invalid")
        })?;
        let (usa_offset, usa_count) = (
            header.update_sequence_offset as usize,
            header.update_sequence_length as usize,
        );
        usa_fixup::apply_usa_fixup(number, data, usa_offset, usa_count)?;
        Ok(FileRecord {
            data,
            header,
            number,
        })
    }

    /// Return whether the record is marked in use.
    pub fn is_used(&self) -> bool {
        self.header.flags & flags::IN_USE != 0
    }

    /// Return whether the record is marked as a directory.
    pub fn is_directory(&self) -> bool {
        self.header.flags & flags::IS_DIRECTORY != 0
    }

    /// Return the hard-link count stored in the header.
    pub fn link_count(&self) -> u16 {
        self.header.link_count
    }

    /// Return the record sequence number.
    pub fn sequence_value(&self) -> u16 {
        self.header.sequence_value
    }

    /// Return the base-record reference for extension records.
    pub fn base_reference(&self) -> u64 {
        self.header.base_reference
    }

    /// Reconstruct the full file reference for this record.
    pub fn file_reference(&self) -> u64 {
        let seq = self.sequence_value() as u64;
        (seq << 48) | (self.number & 0x0000_FFFF_FFFF_FFFF)
    }

    /// Return the `(attributes_offset, used_size)` bounds for attribute walking.
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
        buf[..size_of::<FileRecordHeader>()].copy_from_slice(header.as_bytes());
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
        {
            let rec = FileRecord::parse(123, None, &mut buf).expect("parse ok");
            assert!(rec.is_used());
            assert_eq!(rec.link_count(), 1);
            let seq = rec.sequence_value() as u64;
            assert_eq!(rec.file_reference(), (seq << 48) | 123);
        }
        // After fixup the sector trailers must contain the replacements.
        assert_eq!(buf[510], 0x11);
        assert_eq!(buf[511], 0x22);
        assert_eq!(buf[1022], 0x33);
        assert_eq!(buf[1023], 0x44);
    }

    #[test]
    fn parse_detects_corruption() {
        let mut buf = build_minimal_record();
        buf[510] = 0xFF;
        match FileRecord::parse(7, None, &mut buf) {
            Err(UsnError::FixupMismatch { number: 7 }) => {}
            Err(other) => panic!("expected FixupMismatch, got {other:?}"),
            Ok(_) => panic!("expected FixupMismatch, got Ok"),
        }
    }
}
