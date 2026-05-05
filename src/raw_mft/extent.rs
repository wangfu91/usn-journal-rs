//! Extent map mapping virtual cluster numbers (VCNs) of the `$MFT`
//! `$DATA` attribute to volume byte offsets.

use crate::{errors::UsnError, raw_mft::data_run::DataRun};

#[derive(Debug, Clone)]
pub(crate) struct ExtentSegment {
    /// First VCN covered by this segment.
    pub vcn_start: u64,
    /// Number of clusters in this segment.
    pub clusters: u64,
    /// LCN at `vcn_start`, or `None` for sparse holes.
    pub lcn: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtentMap {
    /// Ordered list of VCN-to-LCN segments.
    pub segments: Vec<ExtentSegment>,
    /// Cluster size in bytes.
    pub cluster_size: u64,
    /// FILE-record size in bytes.
    pub file_record_size: u64,
    /// Total number of logical clusters across all segments.
    pub total_clusters: u64,
}

impl ExtentMap {
    /// Build an extent map from the decoded runs of the `$MFT` data stream.
    pub fn from_runs(runs: &[DataRun], cluster_size: u64, file_record_size: u64) -> Self {
        let mut segments = Vec::with_capacity(runs.len());
        let mut vcn_start: u64 = 0;
        let mut total_clusters: u64 = 0;
        for run in runs {
            match *run {
                DataRun::Data { lcn, clusters } => {
                    segments.push(ExtentSegment {
                        vcn_start,
                        clusters,
                        lcn: Some(lcn),
                    });
                    vcn_start = vcn_start.saturating_add(clusters);
                    total_clusters = total_clusters.saturating_add(clusters);
                }
                DataRun::Sparse { clusters } => {
                    segments.push(ExtentSegment {
                        vcn_start,
                        clusters,
                        lcn: None,
                    });
                    vcn_start = vcn_start.saturating_add(clusters);
                    total_clusters = total_clusters.saturating_add(clusters);
                }
            }
        }
        ExtentMap {
            segments,
            cluster_size,
            file_record_size,
            total_clusters,
        }
    }

    /// Total number of records that the MFT data can hold (including
    /// sparse holes).
    pub fn record_count(&self) -> u64 {
        if self.file_record_size == 0 {
            return 0;
        }
        self.total_clusters.saturating_mul(self.cluster_size) / self.file_record_size
    }

    /// Translate a record number to its absolute volume byte offset.
    /// Returns `Ok(None)` for sparse regions and `Err` for out-of-range
    /// record numbers.
    pub fn record_offset(&self, record_number: u64) -> Result<Option<u64>, UsnError> {
        let byte_off = record_number
            .checked_mul(self.file_record_size)
            .ok_or(UsnError::InvalidDataRun("record offset overflow"))?;
        let vcn = byte_off / self.cluster_size;
        let inner = byte_off % self.cluster_size;
        for seg in &self.segments {
            let end = seg.vcn_start + seg.clusters;
            if vcn >= seg.vcn_start && vcn < end {
                let local_vcn = vcn - seg.vcn_start;
                return Ok(seg
                    .lcn
                    .map(|lcn| (lcn + local_vcn) * self.cluster_size + inner));
            }
        }
        Err(UsnError::InvalidMftRecord {
            number: record_number,
            reason: "record number outside MFT extent map",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_record_numbers_in_single_run() {
        let runs = vec![DataRun::Data {
            lcn: 100,
            clusters: 4,
        }];
        let map = ExtentMap::from_runs(&runs, 4096, 1024);
        // 4 records per cluster
        assert_eq!(map.record_count(), 16);
        assert_eq!(map.record_offset(0).unwrap(), Some(100 * 4096));
        assert_eq!(map.record_offset(3).unwrap(), Some(100 * 4096 + 3 * 1024));
        assert_eq!(map.record_offset(4).unwrap(), Some(101 * 4096));
    }

    #[test]
    fn maps_record_numbers_across_multiple_runs() {
        let runs = vec![
            DataRun::Data {
                lcn: 100,
                clusters: 2,
            }, // VCN 0..2
            DataRun::Data {
                lcn: 200,
                clusters: 3,
            }, // VCN 2..5
        ];
        let map = ExtentMap::from_runs(&runs, 4096, 1024);
        // record 8 -> VCN 2 (start of run 2) -> LCN 200
        assert_eq!(map.record_offset(8).unwrap(), Some(200 * 4096));
        // record 12 -> VCN 3 -> LCN 201
        assert_eq!(map.record_offset(12).unwrap(), Some(201 * 4096));
    }

    #[test]
    fn returns_none_for_sparse_holes() {
        let runs = vec![
            DataRun::Sparse { clusters: 2 },
            DataRun::Data {
                lcn: 500,
                clusters: 1,
            },
        ];
        let map = ExtentMap::from_runs(&runs, 4096, 1024);
        assert_eq!(map.record_offset(0).unwrap(), None);
        assert_eq!(map.record_offset(8).unwrap(), Some(500 * 4096));
    }

    #[test]
    fn out_of_range_returns_error() {
        let runs = vec![DataRun::Data {
            lcn: 100,
            clusters: 1,
        }];
        let map = ExtentMap::from_runs(&runs, 4096, 1024);
        assert!(map.record_offset(100).is_err());
    }
}
