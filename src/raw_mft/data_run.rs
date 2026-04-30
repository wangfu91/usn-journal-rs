//! NTFS data-run decoder.
//!
//! Non-resident attributes' content is described by a sequence of "data
//! runs" that map a contiguous range of virtual cluster numbers (VCNs) to
//! either a logical cluster number (LCN) on the volume or to a sparse
//! hole. Each run is encoded as a header byte (high nibble = byte length
//! of the offset field, low nibble = byte length of the length field)
//! followed by those length and offset fields, the offset being a signed
//! delta from the previous run's LCN.

use crate::errors::UsnError;

/// One decoded data run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRun {
    Data { lcn: u64, clusters: u64 },
    Sparse { clusters: u64 },
}

/// Aggregated information about an attribute's runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataRunSummary {
    pub run_count: u32,
    pub total_clusters: u64,
}

const MAX_FIELD_BYTES: usize = 8;

/// Decode a sequence of data-run records starting at `runs`.
///
/// Returns the sequence of runs alongside a [`DataRunSummary`].
pub(crate) fn decode_runs(runs: &[u8]) -> Result<(Vec<DataRun>, DataRunSummary), UsnError> {
    let mut out: Vec<DataRun> = Vec::new();
    let mut summary = DataRunSummary::default();
    let mut prev_lcn: i128 = 0;
    let mut cursor = 0usize;

    loop {
        if cursor >= runs.len() {
            return Err(UsnError::InvalidDataRun("unterminated data run sequence"));
        }
        let descriptor = runs[cursor];
        if descriptor == 0 {
            break;
        }
        let length_bytes = (descriptor & 0x0F) as usize;
        let offset_bytes = ((descriptor >> 4) & 0x0F) as usize;
        if length_bytes == 0 || length_bytes > MAX_FIELD_BYTES {
            return Err(UsnError::InvalidDataRun("invalid run length field"));
        }
        if offset_bytes > MAX_FIELD_BYTES {
            return Err(UsnError::InvalidDataRun("invalid run offset field"));
        }
        cursor += 1;

        if cursor + length_bytes > runs.len() {
            return Err(UsnError::InvalidDataRun("truncated run length field"));
        }
        let mut len_buf = [0u8; MAX_FIELD_BYTES];
        len_buf[..length_bytes].copy_from_slice(&runs[cursor..cursor + length_bytes]);
        let clusters = u64::from_le_bytes(len_buf);
        if clusters == 0 {
            return Err(UsnError::InvalidDataRun("zero-length run"));
        }
        cursor += length_bytes;

        let lcn_opt = if offset_bytes == 0 {
            None
        } else {
            if cursor + offset_bytes > runs.len() {
                return Err(UsnError::InvalidDataRun("truncated run offset field"));
            }
            let mut off_buf = [0u8; MAX_FIELD_BYTES];
            off_buf[..offset_bytes].copy_from_slice(&runs[cursor..cursor + offset_bytes]);
            let raw = i64::from_le_bytes(off_buf);
            // Sign-extend from offset_bytes*8 bits.
            let empty_bits = (MAX_FIELD_BYTES - offset_bytes) * 8;
            let delta = (raw << empty_bits) >> empty_bits;
            cursor += offset_bytes;
            let new_lcn = prev_lcn
                .checked_add(delta as i128)
                .ok_or(UsnError::InvalidDataRun("relative offset overflow"))?;
            if new_lcn < 0 {
                return Err(UsnError::InvalidDataRun("relative offset underflow"));
            }
            prev_lcn = new_lcn;
            Some(new_lcn as u64)
        };

        summary.run_count = summary.run_count.saturating_add(1);
        summary.total_clusters = summary.total_clusters.saturating_add(clusters);

        match lcn_opt {
            Some(lcn) => out.push(DataRun::Data { lcn, clusters }),
            None => out.push(DataRun::Sparse { clusters }),
        }
    }

    Ok((out, summary))
}

/// Decode runs but only produce the summary, without allocating a `Vec`
/// for each individual run. Used in the hot path of `RawMftEntry`
/// construction where the per-run details aren't needed.
pub(crate) fn summarize_runs(runs: &[u8]) -> Result<DataRunSummary, UsnError> {
    let mut summary = DataRunSummary::default();
    let mut prev_lcn: i128 = 0;
    let mut cursor = 0usize;

    loop {
        if cursor >= runs.len() {
            return Err(UsnError::InvalidDataRun("unterminated data run sequence"));
        }
        let descriptor = runs[cursor];
        if descriptor == 0 {
            break;
        }
        let length_bytes = (descriptor & 0x0F) as usize;
        let offset_bytes = ((descriptor >> 4) & 0x0F) as usize;
        if length_bytes == 0 || length_bytes > MAX_FIELD_BYTES {
            return Err(UsnError::InvalidDataRun("invalid run length field"));
        }
        if offset_bytes > MAX_FIELD_BYTES {
            return Err(UsnError::InvalidDataRun("invalid run offset field"));
        }
        cursor += 1;

        if cursor + length_bytes > runs.len() {
            return Err(UsnError::InvalidDataRun("truncated run length field"));
        }
        let mut len_buf = [0u8; MAX_FIELD_BYTES];
        len_buf[..length_bytes].copy_from_slice(&runs[cursor..cursor + length_bytes]);
        let clusters = u64::from_le_bytes(len_buf);
        if clusters == 0 {
            return Err(UsnError::InvalidDataRun("zero-length run"));
        }
        cursor += length_bytes;

        if offset_bytes != 0 {
            if cursor + offset_bytes > runs.len() {
                return Err(UsnError::InvalidDataRun("truncated run offset field"));
            }
            let mut off_buf = [0u8; MAX_FIELD_BYTES];
            off_buf[..offset_bytes].copy_from_slice(&runs[cursor..cursor + offset_bytes]);
            let raw = i64::from_le_bytes(off_buf);
            let empty_bits = (MAX_FIELD_BYTES - offset_bytes) * 8;
            let delta = (raw << empty_bits) >> empty_bits;
            cursor += offset_bytes;
            let new_lcn = prev_lcn
                .checked_add(delta as i128)
                .ok_or(UsnError::InvalidDataRun("relative offset overflow"))?;
            if new_lcn < 0 {
                return Err(UsnError::InvalidDataRun("relative offset underflow"));
            }
            prev_lcn = new_lcn;
        }

        summary.run_count = summary.run_count.saturating_add(1);
        summary.total_clusters = summary.total_clusters.saturating_add(clusters);
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_single_run() {
        // header 0x21: 1 byte length, 2 byte offset
        // length = 0x05, offset = 0x0234 -> LCN 564
        let runs = [0x21u8, 0x05, 0x34, 0x02, 0x00];
        let (rs, sum) = decode_runs(&runs).unwrap();
        assert_eq!(rs, vec![DataRun::Data { lcn: 564, clusters: 5 }]);
        assert_eq!(sum.run_count, 1);
        assert_eq!(sum.total_clusters, 5);
    }

    #[test]
    fn decodes_relative_offset_chain() {
        // first run: 0x21 len=5 off=10 -> lcn 10
        // second run: 0x21 len=3 off=-3 -> lcn 7
        let runs = [0x21u8, 0x05, 0x0A, 0x00, 0x21, 0x03, 0xFD, 0xFF, 0x00];
        let (rs, _sum) = decode_runs(&runs).unwrap();
        assert_eq!(
            rs,
            vec![
                DataRun::Data { lcn: 10, clusters: 5 },
                DataRun::Data { lcn: 7, clusters: 3 },
            ]
        );
    }

    #[test]
    fn decodes_sparse_run() {
        // header 0x01: 1 byte length, 0 byte offset -> sparse hole
        let runs = [0x01u8, 0x07, 0x00];
        let (rs, sum) = decode_runs(&runs).unwrap();
        assert_eq!(rs, vec![DataRun::Sparse { clusters: 7 }]);
        assert_eq!(sum.total_clusters, 7);
    }

    #[test]
    fn rejects_truncated_runs() {
        let runs = [0x21u8, 0x05]; // missing offset bytes + terminator
        assert!(decode_runs(&runs).is_err());
    }

    #[test]
    fn rejects_zero_length_run() {
        let runs = [0x11u8, 0x00, 0x00, 0x00];
        assert!(decode_runs(&runs).is_err());
    }

    #[test]
    fn rejects_negative_absolute_lcn() {
        // header 0x11 len=1 off=-5 -> previous lcn = 0, new lcn = -5
        let runs = [0x11u8, 0x01, 0xFB, 0x00];
        assert!(decode_runs(&runs).is_err());
    }
}
