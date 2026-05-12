//! Integration test: parallel raw-MFT chunk parsing matches the serial chunk path.
//!
//! The test intentionally samples an early, relatively stable prefix of the MFT
//! so live-filesystem churn is less likely to invalidate the comparison.

use std::num::{NonZeroU64, NonZeroUsize};

use usn_journal_rs::{
    Fid,
    errors::UsnError,
    raw_mft::{RawMft, RawMftBatchEntry, RawMftChunkPlanOptions, RawMftScanOptions},
    volume::Volume,
};

fn pick_drive() -> char {
    std::env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

#[test]
fn raw_mft_parallel_chunk_path_matches_serial_chunk_path() -> Result<(), String> {
    let drive = pick_drive();
    let volume = match Volume::from_drive_letter(drive) {
        Ok(volume) => volume,
        Err(UsnError::NotElevated) => {
            eprintln!("raw_mft_parallel_chunk_consistency: skipping (requires admin privileges)");
            return Ok(());
        }
        Err(error) => {
            eprintln!("raw_mft_parallel_chunk_consistency: skipping on {drive}: {error}");
            return Ok(());
        }
    };

    let raw_mft = match RawMft::new(&volume) {
        Ok(raw_mft) => raw_mft,
        Err(UsnError::UnsupportedFilesystem(message)) => {
            eprintln!(
                "raw_mft_parallel_chunk_consistency: skipping (unsupported filesystem on {drive}: {message})"
            );
            return Ok(());
        }
        Err(error) => return Err(format!("RawMft::new failed on {drive}: {error}")),
    };

    let Some(max_records_per_chunk) = NonZeroU64::new(1024) else {
        return Err("max_records_per_chunk must be non-zero".into());
    };
    let chunk_options = RawMftChunkPlanOptions::builder()
        .start_record(24)
        .end_record(Some(50_000))
        .max_records_per_chunk(max_records_per_chunk)
        .build();
    let chunks: Vec<_> = raw_mft
        .plan_chunks_with_options(chunk_options)
        .into_iter()
        .take(8)
        .collect();
    if chunks.is_empty() {
        eprintln!("raw_mft_parallel_chunk_consistency: skipping (no work chunks in sampled range)");
        return Ok(());
    }

    let options = RawMftScanOptions::builder()
        .collect_alternate_data_streams(false)
        .collect_data_run_summary(false)
        .build();
    let serial_entries: Vec<_> = chunks
        .iter()
        .map(|chunk| {
            raw_mft
                .read_chunk(*chunk, options.clone())
                .map_err(|error| format!("serial read_chunk failed for {:?}: {error}", chunk))
        })
        .collect::<Result<_, _>>()?;

    let Some(worker_count) = NonZeroUsize::new(4) else {
        return Err("worker_count must be non-zero".into());
    };
    let parallel_batches = raw_mft
        .parallel()
        .chunks(chunks.clone())
        .scan_options(options)
        .workers(worker_count)
        .collect_batches()
        .map_err(|error| format!("parallel collect_batches failed: {error}"))?;

    assert_eq!(
        parallel_batches.len(),
        chunks.len(),
        "parallel batch count must match planned chunk count"
    );

    for ((chunk, serial_entries), parallel_batch) in chunks
        .iter()
        .zip(serial_entries.iter())
        .zip(parallel_batches.iter())
    {
        assert_eq!(
            *chunk, parallel_batch.chunk,
            "parallel output must preserve chunk order"
        );
        let serial_identity: Vec<_> = serial_entries.iter().map(entry_identity).collect();
        let parallel_identity: Vec<_> = parallel_batch.entries.iter().map(entry_identity).collect();
        assert_eq!(
            serial_identity, parallel_identity,
            "parallel chunk identity/order must match serial chunk output for {:?}",
            chunk
        );
    }

    Ok(())
}

fn entry_identity(entry: &RawMftBatchEntry) -> (u64, Fid, u64, Fid, bool) {
    (
        entry.record_number,
        entry.file_reference,
        entry.base_record_reference,
        entry.parent_reference,
        entry.is_directory,
    )
}
