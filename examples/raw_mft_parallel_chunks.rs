use std::{
    env,
    error::Error,
    num::{NonZeroU64, NonZeroUsize},
    thread,
    time::Instant,
};

use usn_journal_rs::{
    raw_mft::{RawMft, RawMftChunkPlanOptions, RawMftScanOptions},
    volume::Volume,
};

fn main() -> Result<(), Box<dyn Error>> {
    let drive = env::args()
        .nth(1)
        .and_then(|arg| arg.chars().next())
        .map(|ch| ch.to_ascii_uppercase())
        .unwrap_or('C');
    let max_records_per_chunk = env::args()
        .nth(2)
        .and_then(|arg| arg.parse::<u64>().ok())
        .and_then(NonZeroU64::new)
        .unwrap_or(NonZeroU64::new(16 * 1024).unwrap_or(NonZeroU64::MIN));
    let worker_count = env::args()
        .nth(3)
        .and_then(|arg| arg.parse::<usize>().ok())
        .and_then(NonZeroUsize::new)
        .unwrap_or(
            thread::available_parallelism()
                .ok()
                .unwrap_or(NonZeroUsize::MIN),
        );

    let volume = Volume::from_drive_letter(drive)?;
    let raw_mft = RawMft::new(&volume)?;
    let chunk_plan = RawMftChunkPlanOptions::builder()
        .max_records_per_chunk(max_records_per_chunk)
        .build();
    let chunks = raw_mft.plan_chunks_with_options(chunk_plan);
    let options = RawMftScanOptions::builder()
        .collect_alternate_data_streams(false)
        .collect_data_run_summary(false)
        .build();

    let start = Instant::now();
    let chunk_count = chunks.len();
    let mut entry_count = 0usize;
    raw_mft
        .parallel()
        .chunks(chunks)
        .scan_options(options)
        .workers(worker_count)
        .for_each_batch(|batch| {
            entry_count += batch.entries.len();
            Ok(())
        })?;
    let elapsed = start.elapsed();

    println!("raw_mft parallel chunks");
    println!("  drive:                    {drive}:");
    println!("  worker_count:             {}", worker_count);
    println!("  max_records_per_chunk:    {}", max_records_per_chunk);
    println!("  chunk_count:              {chunk_count}");
    println!("  entry_count:              {entry_count}");
    println!("  elapsed:                  {:.3}s", elapsed.as_secs_f64());
    if elapsed.as_secs_f64() > 0.0 {
        println!(
            "  entries_per_second:       {:.0}",
            entry_count as f64 / elapsed.as_secs_f64()
        );
    }

    Ok(())
}
