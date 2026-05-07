use std::num::NonZeroU64;

use crate::raw_mft::record::FIRST_NORMAL_RECORD;

/// A deterministic logical record range for raw `$MFT` parsing work.
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
pub struct RawMftWorkPlanOptions {
    /// Whether unused records should be dropped using the `$MFT` bitmap.
    pub skip_unused: bool,
    /// Inclusive first record number to consider.
    pub start_record: u64,
    /// Exclusive end record number to consider. `None` means `record_count()`.
    pub end_record: Option<u64>,
    /// Maximum logical records per chunk.
    pub max_records_per_chunk: NonZeroU64,
}

impl Default for RawMftWorkPlanOptions {
    fn default() -> Self {
        Self {
            skip_unused: true,
            start_record: FIRST_NORMAL_RECORD,
            max_records_per_chunk: NonZeroU64::new(16 * 1024).unwrap_or(NonZeroU64::MIN),
            end_record: None,
        }
    }
}

/// Build logical work chunks from a record-usage predicate.
pub(crate) fn build_work_chunks<F>(
    start_record: u64,
    end_record: u64,
    max_records_per_chunk: NonZeroU64,
    skip_unused: bool,
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
    let mut current_start: Option<u64> = None;

    for record_number in start_record..end_record {
        let record_is_used = !skip_unused || is_used(record_number);
        if !record_is_used {
            if let Some(start) = current_start.take() {
                chunks.push(RawMftWorkChunk {
                    start_record: start,
                    end_record: record_number,
                });
            }
            continue;
        }

        match current_start {
            Some(start) if record_number.saturating_sub(start) >= max_records_per_chunk => {
                chunks.push(RawMftWorkChunk {
                    start_record: start,
                    end_record: record_number,
                });
                current_start = Some(record_number);
            }
            Some(_) => {}
            None => current_start = Some(record_number),
        }
    }

    if let Some(start) = current_start {
        chunks.push(RawMftWorkChunk {
            start_record: start,
            end_record,
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::{RawMftWorkChunk, build_work_chunks};
    use std::num::NonZeroU64;

    #[test]
    fn skip_unused_coalesces_only_used_runs() -> Result<(), String> {
        let Some(chunk_size) = NonZeroU64::new(8) else {
            return Err("chunk size must be non-zero".into());
        };
        let used = [false, true, true, false, true, true, true, false];
        let chunks =
            build_work_chunks(0, used.len() as u64, chunk_size, true, |n| used[n as usize]);
        assert_eq!(
            chunks,
            vec![
                RawMftWorkChunk {
                    start_record: 1,
                    end_record: 3,
                },
                RawMftWorkChunk {
                    start_record: 4,
                    end_record: 7,
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
}
