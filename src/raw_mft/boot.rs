//! NTFS boot sector parsing.
//!
//! The boot sector is the first 512-byte sector of an NTFS volume; it
//! contains the geometry needed to locate the `$MFT`.

use crate::errors::UsnError;

/// Size of an NTFS sector in bytes (always 512 for the boot sector itself).
pub const BOOT_SECTOR_SIZE: usize = 512;

/// Expected OEM ID for an NTFS volume (`"NTFS    "`).
pub const NTFS_OEM_ID: &[u8; 8] = b"NTFS    ";

/// Parsed NTFS boot sector geometry.
#[derive(Debug, Clone)]
pub(crate) struct BootSector {
    pub bytes_per_sector: u32,
    pub sectors_per_cluster: u32,
    pub total_sectors: u64,
    pub mft_lcn: u64,
    pub mft_mirror_lcn: u64,
    pub file_record_size: u64,
    pub cluster_size: u64,
    pub mft_byte_offset: u64,
}

impl BootSector {
    /// Parse a 512-byte boot sector buffer.
    ///
    /// Returns [`UsnError::UnsupportedFilesystem`] when the OEM ID does not
    /// indicate an NTFS volume (e.g. ReFS), and
    /// [`UsnError::InvalidBootSector`] when the boot sector geometry is
    /// outside reasonable NTFS bounds.
    pub fn parse(buf: &[u8]) -> Result<Self, UsnError> {
        if buf.len() < BOOT_SECTOR_SIZE {
            return Err(UsnError::InvalidBootSector("buffer smaller than 512 bytes"));
        }

        // OEM ID lives at offset 3 and is 8 bytes long.
        let oem = &buf[3..11];
        if oem != NTFS_OEM_ID {
            return Err(UsnError::UnsupportedFilesystem(
                "volume is not NTFS (OEM ID mismatch)",
            ));
        }

        let bytes_per_sector = u16::from_le_bytes([buf[11], buf[12]]) as u32;
        let sectors_per_cluster_raw = buf[13] as i8;
        let total_sectors = u64::from_le_bytes(buf[40..48].try_into().unwrap());
        let mft_lcn = u64::from_le_bytes(buf[48..56].try_into().unwrap());
        let mft_mirror_lcn = u64::from_le_bytes(buf[56..64].try_into().unwrap());
        let file_record_size_info = buf[64] as i8;

        if bytes_per_sector == 0 || !bytes_per_sector.is_power_of_two() {
            return Err(UsnError::InvalidBootSector(
                "bytes_per_sector must be a non-zero power of two",
            ));
        }

        let sectors_per_cluster: u32 = if sectors_per_cluster_raw > 0 {
            sectors_per_cluster_raw as u32
        } else if sectors_per_cluster_raw < 0 {
            let exp = (-(sectors_per_cluster_raw as i32)) as u32;
            if exp >= 32 {
                return Err(UsnError::InvalidBootSector(
                    "sectors_per_cluster exponent too large",
                ));
            }
            let cluster_size = 1u32 << exp;
            if cluster_size < bytes_per_sector {
                return Err(UsnError::InvalidBootSector(
                    "cluster size smaller than sector size",
                ));
            }
            cluster_size / bytes_per_sector
        } else {
            return Err(UsnError::InvalidBootSector(
                "sectors_per_cluster cannot be zero",
            ));
        };

        let cluster_size = (bytes_per_sector as u64)
            .checked_mul(sectors_per_cluster as u64)
            .ok_or(UsnError::InvalidBootSector("cluster size overflow"))?;

        let file_record_size: u64 = if file_record_size_info > 0 {
            (file_record_size_info as u64)
                .checked_mul(cluster_size)
                .ok_or(UsnError::InvalidBootSector("file_record_size overflow"))?
        } else if file_record_size_info < 0 {
            let exp = (-(file_record_size_info as i32)) as u32;
            if exp >= 32 {
                return Err(UsnError::InvalidBootSector(
                    "file_record_size exponent too large",
                ));
            }
            1u64 << exp
        } else {
            return Err(UsnError::InvalidBootSector(
                "file_record_size_info cannot be zero",
            ));
        };

        if !(256..=65_536).contains(&file_record_size) {
            return Err(UsnError::InvalidBootSector(
                "file_record_size outside reasonable bounds",
            ));
        }
        if (file_record_size as u32) % bytes_per_sector != 0 {
            return Err(UsnError::InvalidBootSector(
                "file_record_size not a multiple of bytes_per_sector",
            ));
        }

        let mft_byte_offset = mft_lcn
            .checked_mul(cluster_size)
            .ok_or(UsnError::InvalidBootSector("MFT offset overflow"))?;

        Ok(BootSector {
            bytes_per_sector,
            sectors_per_cluster,
            total_sectors,
            mft_lcn,
            mft_mirror_lcn,
            file_record_size,
            cluster_size,
            mft_byte_offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_boot_sector(
        bytes_per_sector: u16,
        sectors_per_cluster: i8,
        total_sectors: u64,
        mft_lcn: u64,
        mft_mirror_lcn: u64,
        file_record_size_info: i8,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; BOOT_SECTOR_SIZE];
        buf[0] = 0xEB;
        buf[1] = 0x52;
        buf[2] = 0x90;
        buf[3..11].copy_from_slice(NTFS_OEM_ID);
        buf[11..13].copy_from_slice(&bytes_per_sector.to_le_bytes());
        buf[13] = sectors_per_cluster as u8;
        buf[40..48].copy_from_slice(&total_sectors.to_le_bytes());
        buf[48..56].copy_from_slice(&mft_lcn.to_le_bytes());
        buf[56..64].copy_from_slice(&mft_mirror_lcn.to_le_bytes());
        buf[64] = file_record_size_info as u8;
        buf
    }

    #[test]
    fn parses_typical_ntfs_boot_sector() {
        let buf = make_boot_sector(512, 8, 0x10_0000, 0xC_0000, 0x2, -10);
        let bs = BootSector::parse(&buf).expect("should parse");
        assert_eq!(bs.bytes_per_sector, 512);
        assert_eq!(bs.sectors_per_cluster, 8);
        assert_eq!(bs.cluster_size, 4096);
        assert_eq!(bs.file_record_size, 1024);
        assert_eq!(bs.mft_lcn, 0xC_0000);
        assert_eq!(bs.mft_byte_offset, 0xC_0000 * 4096);
    }

    #[test]
    fn parses_positive_file_record_size() {
        let buf = make_boot_sector(512, 1, 1000, 100, 200, 2);
        let bs = BootSector::parse(&buf).expect("should parse");
        assert_eq!(bs.cluster_size, 512);
        assert_eq!(bs.file_record_size, 1024);
    }

    #[test]
    fn rejects_non_ntfs_oem_id() {
        let mut buf = make_boot_sector(512, 8, 0, 0, 0, -10);
        buf[3..11].copy_from_slice(b"ReFS    ");
        assert!(matches!(
            BootSector::parse(&buf),
            Err(UsnError::UnsupportedFilesystem(_))
        ));
    }

    #[test]
    fn rejects_short_buffer() {
        let buf = vec![0u8; 100];
        assert!(matches!(
            BootSector::parse(&buf),
            Err(UsnError::InvalidBootSector(_))
        ));
    }

    #[test]
    fn rejects_zero_sectors_per_cluster() {
        let buf = make_boot_sector(512, 0, 0, 0, 0, -10);
        assert!(matches!(
            BootSector::parse(&buf),
            Err(UsnError::InvalidBootSector(_))
        ));
    }

    #[test]
    fn rejects_zero_file_record_size_info() {
        let buf = make_boot_sector(512, 8, 0, 0, 0, 0);
        assert!(matches!(
            BootSector::parse(&buf),
            Err(UsnError::InvalidBootSector(_))
        ));
    }

    #[test]
    fn rejects_record_size_out_of_bounds() {
        let buf = make_boot_sector(512, 8, 0, 0, 0, -7);
        assert!(matches!(
            BootSector::parse(&buf),
            Err(UsnError::InvalidBootSector(_))
        ));
    }
}
