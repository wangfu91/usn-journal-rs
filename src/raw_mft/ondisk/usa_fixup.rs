//! Update Sequence Array (USA) "fixup" handling for NTFS multi-sector
//! transfer-protected records.
//!
//! Every NTFS multi-sector record (FILE record, INDX block, …) ends each
//! 512-byte sector with a 2-byte sentinel value. The real bytes that used
//! to live in those trailer positions are stored in the Update Sequence
//! Array (USA) near the start of the record. Fixup is therefore a two-step
//! process: first verify that every sector trailer still contains the
//! sentinel, then copy the saved replacement bytes back into place.
//! MFT FILE record (e.g. 1024 bytes total)
//!     0x0000  +------------------------------------------------------+
//!             | FILE header                                          |
//!             |  - signature "FILE"                                  |
//!             |  - usa_offset                                        |
//!             |  - usa_count                                         |
//!             |  - lsn                                               |
//!             |  - sequence_number                                   |
//!             |  - hard_link_count                                   |
//!             |  - first_attribute_offset                            |
//!             |  - flags                                             |
//!             |  - used_size                                         |
//!             |  - allocated_size                                    |
//!             |  - base_file_reference                               |
//!             |  - next_attribute_id                                 |
//!             |  - mft_record_number                                 |
//!             +------------------------------------------------------+
//!     0x00??  | USA array                                            |
//!             |  - usa[0] = update sequence number                   |
//!             |  - usa[1..] = original tail bytes per sector         |
//!             +------------------------------------------------------+
//!     0x00??  | Attribute #1                                         |
//!             |  - type / len / flags / id                           |
//!             |  - resident or non-resident payload                  |
//!             +------------------------------------------------------+
//!     0x00??  | Attribute #2                                         |
//!             +------------------------------------------------------+
//!     0x00??  | Attribute #3                                         |
//!             +------------------------------------------------------+
//!     0x03FC  | End marker = 0xFFFFFFFF                              |
//!             +------------------------------------------------------+
//!

use crate::errors::UsnError;

/// All NTFS USA-protected records use 512-byte logical sectors regardless
/// of the underlying disk's bytes-per-sector.
pub const USA_SECTOR_SIZE: usize = 512;

/// Apply the Update Sequence Array fixup to a multi-sector record buffer.
///
/// `usa_offset` and `usa_count` come from the record header. `usa_count`
/// is the count of `u16` entries stored in the USA: one leading sentinel
/// word plus one saved trailer word for each protected sector.
///
/// Returns [`UsnError::FixupMismatch`] if any sector trailer doesn't
/// match the sentinel, and [`UsnError::InvalidMftRecord`] if the offsets
/// or counts are nonsensical.
pub(crate) fn apply_usa_fixup(
    record_number: u64,
    data: &mut [u8],
    usa_offset: usize,
    usa_count: usize,
) -> Result<(), UsnError> {
    if usa_count < 2 {
        return Err(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "update_sequence_length must be at least 2",
        });
    }

    // `usa_count` is the number of `u16` words in the USA, not the number
    // of sectors. Word 0 stores the shared sentinel, and each remaining
    // word stores one protected sector's original 2-byte trailer. So the
    // number of protected sectors is `usa_count - 1`.
    let sectors = usa_count - 1;
    // Each usa entry is 2 bytes, so the USA occupies `2 * usa_count` bytes starting at `usa_offset`. 
    // Verify that the USA fits within the record buffer before we try to read from it.
    let usa_end = usa_offset
        .checked_add(usa_count.checked_mul(2).ok_or(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "USA size overflow",
        })?)
        .ok_or(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "USA range overflow",
        })?;
    if usa_end > data.len() {
        return Err(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "USA extends past record",
        });
    }
    // The USA claims that `sectors` protected 512-byte sectors exist in
    // this record. That implies the buffer must be large enough to reach
    // every sector trailer; otherwise the header is inconsistent or the
    // record was truncated before we finished reading it.
    let needed = sectors
        .checked_mul(USA_SECTOR_SIZE)
        .ok_or(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "sector range overflow",
        })?;
    if needed > data.len() {
        return Err(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "record smaller than declared sector count",
        });
    }

    // The first USA word is the sentinel/USN. NTFS copies that same
    // 2-byte value into the last two bytes of every protected sector so
    // we can verify that the record was written as a complete unit before
    // trusting any of the attribute data inside it.
    let sentinel = [data[usa_offset], data[usa_offset + 1]];

    for i in 0..sectors {
        // USA layout: word 0 is the sentinel, words 1..N are the original
        // trailer bytes for sectors 0..N-1. Each loop iteration restores
        // one sector trailer from the matching USA entry.
        let entry_off = usa_offset + 2 + i * 2;
        let replacement = [data[entry_off], data[entry_off + 1]];
        // Sector trailers sit at the last two bytes of each 512-byte
        // protected sector, so the offset is predictable once we know the
        // sector index.
        let trailer_off = (i + 1) * USA_SECTOR_SIZE - 2;
        if trailer_off + 2 > data.len() {
            return Err(UsnError::InvalidMftRecord {
                number: record_number,
                reason: "sector trailer past buffer",
            });
        }
        if data[trailer_off] != sentinel[0] || data[trailer_off + 1] != sentinel[1] {
            return Err(UsnError::FixupMismatch {
                number: record_number,
            });
        }
        data[trailer_off] = replacement[0];
        data[trailer_off + 1] = replacement[1];
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_record(
        record_size: usize,
        usa_offset: usize,
        sentinel: [u8; 2],
        replacements: &[[u8; 2]],
    ) -> Vec<u8> {
        let mut buf = vec![0u8; record_size];
        // Lay out the USA exactly as NTFS does in a real record:
        // [sentinel][saved trailer word for sector 0][saved trailer word for sector 1]...
        buf[usa_offset..usa_offset + 2].copy_from_slice(&sentinel);
        for (i, r) in replacements.iter().enumerate() {
            let off = usa_offset + 2 + i * 2;
            buf[off..off + 2].copy_from_slice(r);
        }
        // Pretend each protected sector already has the USA sentinel in
        // its trailer so the fixup logic can validate and restore it.
        for i in 0..replacements.len() {
            let trailer = (i + 1) * USA_SECTOR_SIZE - 2;
            buf[trailer..trailer + 2].copy_from_slice(&sentinel);
        }
        buf
    }

    #[test]
    fn applies_fixup_for_two_sectors() {
        let replacements = [[0xAA, 0xBB], [0xCC, 0xDD]];
        let mut buf = build_record(1024, 42, [0x01, 0x00], &replacements);
        apply_usa_fixup(7, &mut buf, 42, 3).expect("fixup ok");
        assert_eq!(&buf[510..512], &replacements[0]);
        assert_eq!(&buf[1022..1024], &replacements[1]);
    }

    #[test]
    fn detects_sector_corruption() {
        let replacements = [[0xAA, 0xBB], [0xCC, 0xDD]];
        let mut buf = build_record(1024, 42, [0x01, 0x00], &replacements);
        // Corrupt sector trailer.
        buf[1022] = 0xFF;
        match apply_usa_fixup(7, &mut buf, 42, 3) {
            Err(UsnError::FixupMismatch { number: 7 }) => {}
            other => panic!("expected FixupMismatch, got {:?}", other),
        }
    }

    #[test]
    fn rejects_too_small_usa_count() {
        let mut buf = vec![0u8; 1024];
        assert!(matches!(
            apply_usa_fixup(1, &mut buf, 0, 1),
            Err(UsnError::InvalidMftRecord { .. })
        ));
    }

    #[test]
    fn rejects_usa_past_buffer() {
        let mut buf = vec![0u8; 1024];
        assert!(matches!(
            apply_usa_fixup(1, &mut buf, 1020, 5),
            Err(UsnError::InvalidMftRecord { .. })
        ));
    }
}
