//! Update Sequence Array (USA) "fixup" handling for NTFS multi-sector
//! transfer-protected records.
//!
//! Every NTFS multi-sector record (FILE record, INDX block, …) ends every
//! 512-byte sector with a 2-byte "USN" sentinel. The real values that
//! belong at those positions are stored in the Update Sequence Array at
//! the start of the record. After validating that every sector trailer
//! matches the sentinel we substitute the real values back in.

use crate::errors::UsnError;

/// All NTFS USA-protected records use 512-byte logical sectors regardless
/// of the underlying disk's bytes-per-sector.
pub const USA_SECTOR_SIZE: usize = 512;

/// Apply the Update Sequence Array fixup to a multi-sector record buffer.
///
/// `usa_offset` and `usa_count` come from the record header. `usa_count`
/// is the count of `u16` entries: a leading 2-byte sentinel followed by
/// one replacement word per protected sector.
///
/// Returns [`UsnError::FixupMismatch`] if any sector trailer doesn't
/// match the sentinel, and [`UsnError::InvalidMftRecord`] if the offsets
/// or counts are nonsensical.
pub(crate) fn apply_fixup(
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
    let sectors = usa_count - 1;
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

    let sentinel = [data[usa_offset], data[usa_offset + 1]];

    for i in 0..sectors {
        let entry_off = usa_offset + 2 + i * 2;
        let replacement = [data[entry_off], data[entry_off + 1]];
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
        // USA layout: sentinel + replacements
        buf[usa_offset..usa_offset + 2].copy_from_slice(&sentinel);
        for (i, r) in replacements.iter().enumerate() {
            let off = usa_offset + 2 + i * 2;
            buf[off..off + 2].copy_from_slice(r);
        }
        // Place sentinel at every sector trailer
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
        apply_fixup(7, &mut buf, 42, 3).expect("fixup ok");
        assert_eq!(&buf[510..512], &replacements[0]);
        assert_eq!(&buf[1022..1024], &replacements[1]);
    }

    #[test]
    fn detects_sector_corruption() {
        let replacements = [[0xAA, 0xBB], [0xCC, 0xDD]];
        let mut buf = build_record(1024, 42, [0x01, 0x00], &replacements);
        // Corrupt sector trailer.
        buf[1022] = 0xFF;
        match apply_fixup(7, &mut buf, 42, 3) {
            Err(UsnError::FixupMismatch { number: 7 }) => {}
            other => panic!("expected FixupMismatch, got {:?}", other),
        }
    }

    #[test]
    fn rejects_too_small_usa_count() {
        let mut buf = vec![0u8; 1024];
        assert!(matches!(
            apply_fixup(1, &mut buf, 0, 1),
            Err(UsnError::InvalidMftRecord { .. })
        ));
    }

    #[test]
    fn rejects_usa_past_buffer() {
        let mut buf = vec![0u8; 1024];
        assert!(matches!(
            apply_fixup(1, &mut buf, 1020, 5),
            Err(UsnError::InvalidMftRecord { .. })
        ));
    }
}
