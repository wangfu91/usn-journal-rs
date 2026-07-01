//! Deterministic logical work-chunk planning for raw-MFT scans.

use std::num::NonZeroU64;

use crate::raw_mft::{layout::record::FIRST_NORMAL_RECORD, options::RawMftRecordRange};

/// A deterministic logical record range for raw `$MFT` parsing work. [start_record, end_record)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawMftWorkChunk {
    /// Inclusive first record number in the chunk.
    pub start_record: u64,
    /// Exclusive end record number in the chunk.
    pub end_record: u64,
}

impl RawMftWorkChunk {
    /// Number of records covered by the chunk.
    #[must_use]
    pub fn record_len(&self) -> u64 {
        self.end_record.saturating_sub(self.start_record)
    }
}

/// Options controlling logical work-chunk planning for raw `$MFT` parsing.
#[derive(Debug, Clone)]
pub struct RawMftChunkPlanOptions {
    /// Whether fully unused logical chunk bands should be retained instead of
    /// being dropped via the `$MFT` bitmap.
    pub(crate) include_unused_records: bool,
    /// Logical record range to consider.
    pub(crate) range: RawMftRecordRange,
    /// Maximum logical records per chunk.
    pub(crate) max_records_per_chunk: NonZeroU64,
}

impl Default for RawMftChunkPlanOptions {
    fn default() -> Self {
        Self {
            include_unused_records: false,
            range: RawMftRecordRange::new(FIRST_NORMAL_RECORD, None),
            max_records_per_chunk: NonZeroU64::new(16 * 1024).unwrap_or(NonZeroU64::MIN),
        }
    }
}

impl RawMftChunkPlanOptions {
    /// Returns a fluent builder for [`RawMftChunkPlanOptions`].
    pub fn builder() -> RawMftChunkPlanOptionsBuilder {
        RawMftChunkPlanOptionsBuilder::default()
    }

    /// Whether fully unused logical chunk bands are retained in the plan.
    #[must_use]
    pub const fn include_unused_records(&self) -> bool {
        self.include_unused_records
    }

    /// Logical record range to consider.
    #[must_use]
    pub const fn range(&self) -> RawMftRecordRange {
        self.range
    }

    /// Maximum logical records per chunk.
    #[must_use]
    pub const fn max_records_per_chunk(&self) -> NonZeroU64 {
        self.max_records_per_chunk
    }
}

/// Fluent builder for [`RawMftChunkPlanOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct RawMftChunkPlanOptionsBuilder {
    inner: RawMftChunkPlanOptions,
}

impl RawMftChunkPlanOptionsBuilder {
    /// Whether fully unused logical chunk bands should be retained in the plan.
    pub fn include_unused_records(mut self, v: bool) -> Self {
        self.inner.include_unused_records = v;
        self
    }

    /// Set the logical record range to consider.
    pub fn range(mut self, v: RawMftRecordRange) -> Self {
        self.inner.range = v;
        self
    }

    /// Set the inclusive first record number to consider.
    pub fn start_record(mut self, v: u64) -> Self {
        self.inner.range.start_record = v;
        self
    }

    /// Set the exclusive end record number to consider.
    pub fn end_record(mut self, v: Option<u64>) -> Self {
        self.inner.range.end_record = v;
        self
    }

    /// Set the maximum logical records per chunk.
    pub fn max_records_per_chunk(mut self, v: NonZeroU64) -> Self {
        self.inner.max_records_per_chunk = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> RawMftChunkPlanOptions {
        self.inner
    }
}

/// Build logical work chunks from a record-usage predicate.
///
/// Chunk sizing always follows the logical record range so worker tasks stay
/// coarse enough for parallel scheduling and buffered volume reads. When
/// `include_unused_records` is `false`, bands that contain no used records at
/// all are omitted; bands with at least one used record are still kept as dense
/// logical ranges and the scan path can skip individual unused records later.
pub(crate) fn build_work_chunks<F>(
    start_record: u64,
    end_record: u64,
    max_records_per_chunk: NonZeroU64,
    include_unused_records: bool,
    mut is_used: F,
) -> Vec<RawMftWorkChunk>
where
    F: FnMut(u64) -> bool,
{
    if start_record >= end_record {
        return Vec::new();
    }

    let max_records_per_chunk = max_records_per_chunk.get();
    let mut chunks = Vec::new();
    let mut chunk_start = start_record;
    while chunk_start < end_record {
        let chunk_end = chunk_start
            .saturating_add(max_records_per_chunk)
            .min(end_record);
        let keep_chunk = include_unused_records || (chunk_start..chunk_end).any(&mut is_used);
        if keep_chunk {
            chunks.push(RawMftWorkChunk {
                start_record: chunk_start,
                end_record: chunk_end,
            });
        }
        chunk_start = chunk_end;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::{RawMftWorkChunk, build_work_chunks};
    use std::num::NonZeroU64;

    #[test]
    fn exclude_unused_records_drops_fully_unused_bands_but_keeps_dense_ranges() -> Result<(), String>
    {
        let Some(chunk_size) = NonZeroU64::new(8) else {
            return Err("chunk size must be non-zero".into());
        };
        let used = [false, true, true, false, true, true, true, false];
        let chunks = build_work_chunks(0, used.len() as u64, chunk_size, false, |n| {
            used[n as usize]
        });
        assert_eq!(
            chunks,
            vec![RawMftWorkChunk {
                start_record: 0,
                end_record: 8,
            }]
        );
        Ok(())
    }

    #[test]
    fn exclude_unused_records_omits_fully_empty_bands() -> Result<(), String> {
        let Some(chunk_size) = NonZeroU64::new(4) else {
            return Err("chunk size must be non-zero".into());
        };
        let used = [
            false, true, false, false, false, false, false, false, true, false, false, false,
        ];
        let chunks = build_work_chunks(0, used.len() as u64, chunk_size, false, |n| {
            used[n as usize]
        });
        assert_eq!(
            chunks,
            vec![
                RawMftWorkChunk {
                    start_record: 0,
                    end_record: 4,
                },
                RawMftWorkChunk {
                    start_record: 8,
                    end_record: 12,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn max_chunk_size_splits_long_used_runs() -> Result<(), String> {
        let Some(chunk_size) = NonZeroU64::new(3) else {
            return Err("chunk size must be non-zero".into());
        };
        let chunks = build_work_chunks(10, 18, chunk_size, false, |_| true);
        assert_eq!(
            chunks,
            vec![
                RawMftWorkChunk {
                    start_record: 10,
                    end_record: 13,
                },
                RawMftWorkChunk {
                    start_record: 13,
                    end_record: 16,
                },
                RawMftWorkChunk {
                    start_record: 16,
                    end_record: 18,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn dense_mode_ignores_usage_gaps() -> Result<(), String> {
        let Some(chunk_size) = NonZeroU64::new(4) else {
            return Err("chunk size must be non-zero".into());
        };
        let used = [true, false, true, false, true, false, true, false];
        let chunks = build_work_chunks(0, used.len() as u64, chunk_size, false, |n| {
            used[n as usize]
        });
        assert_eq!(
            chunks,
            vec![
                RawMftWorkChunk {
                    start_record: 0,
                    end_record: 4,
                },
                RawMftWorkChunk {
                    start_record: 4,
                    end_record: 8,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn empty_range_produces_no_chunks() {
        let chunk_size = NonZeroU64::new(8).expect("non-zero");
        assert!(build_work_chunks(5, 5, chunk_size, true, |_| true).is_empty());
        // A start beyond the end must also produce nothing (no underflow).
        assert!(build_work_chunks(10, 5, chunk_size, true, |_| true).is_empty());
    }

    #[test]
    fn include_unused_keeps_fully_unused_bands() {
        let chunk_size = NonZeroU64::new(4).expect("non-zero");
        // Every record unused, but include_unused_records = true keeps all bands.
        let chunks = build_work_chunks(0, 12, chunk_size, true, |_| false);
        assert_eq!(
            chunks,
            vec![
                RawMftWorkChunk {
                    start_record: 0,
                    end_record: 4,
                },
                RawMftWorkChunk {
                    start_record: 4,
                    end_record: 8,
                },
                RawMftWorkChunk {
                    start_record: 8,
                    end_record: 12,
                },
            ]
        );
    }

    #[test]
    fn range_smaller_than_chunk_yields_single_chunk() {
        let chunk_size = NonZeroU64::new(64).expect("non-zero");
        let chunks = build_work_chunks(3, 9, chunk_size, true, |_| true);
        assert_eq!(
            chunks,
            vec![RawMftWorkChunk {
                start_record: 3,
                end_record: 9,
            }]
        );
    }

    #[test]
    fn work_chunk_record_len_is_saturating() {
        assert_eq!(
            RawMftWorkChunk {
                start_record: 10,
                end_record: 18,
            }
            .record_len(),
            8
        );
        // An inverted chunk must saturate to zero rather than underflow.
        assert_eq!(
            RawMftWorkChunk {
                start_record: 18,
                end_record: 10,
            }
            .record_len(),
            0
        );
    }

    #[test]
    fn chunk_plan_builder_round_trips_values() {
        use crate::raw_mft::{RawMftChunkPlanOptions, RawMftRecordRange};

        let options = RawMftChunkPlanOptions::builder()
            .include_unused_records(true)
            .range(RawMftRecordRange::new(24, Some(1024)))
            .max_records_per_chunk(NonZeroU64::new(2048).expect("non-zero"))
            .build();

        assert!(options.include_unused_records());
        assert_eq!(options.range().start_record(), 24);
        assert_eq!(options.range().end_record(), Some(1024));
        assert_eq!(
            options.max_records_per_chunk(),
            NonZeroU64::new(2048).expect("non-zero")
        );
    }

    #[test]
    fn chunk_plan_default_excludes_unused_from_first_normal_record() {
        use crate::raw_mft::RawMftChunkPlanOptions;

        let options = RawMftChunkPlanOptions::default();
        assert!(!options.include_unused_records());
        assert_eq!(options.range().start_record(), 24);
        assert_eq!(options.range().end_record(), None);
    }
}
