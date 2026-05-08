use std::{error::Error, time::Instant};

use usn_journal_rs::raw_mft::RawMft;

#[allow(dead_code)]
#[path = "..\\benches\\support\\raw_mft_ingest_shared.rs"]
mod raw_mft_ingest_shared;

use raw_mft_ingest_shared::{bench_config, open_volume, print_bench_config, run_parallel_ingest};

fn main() -> Result<(), Box<dyn Error>> {
    let config = bench_config().clone();
    print_bench_config(&config);

    let Some(volume) = open_volume(config.drive) else {
        return Ok(());
    };
    let raw_mft = RawMft::new(&volume)?;

    let start = Instant::now();
    let summary = run_parallel_ingest(&raw_mft, &config)?;
    let elapsed = start.elapsed();

    println!("raw_mft parallel ingest profile");
    println!("  drive:                    {}:", config.drive);
    println!("  worker_count:             {}", config.worker_count);
    println!("  max_records_per_chunk:    {}", config.chunk_records);
    println!("  main_buffer_bytes:        {}", config.main_buffer_bytes);
    println!("  attr_buffer_bytes:        {}", config.attr_buffer_bytes);
    println!("  start_record:             {}", config.start_record);
    println!(
        "  end_record:               {}",
        config
            .end_record
            .map(|value| value.to_string())
            .unwrap_or_else(|| "full".to_owned())
    );
    println!("  records:                  {}", summary.records);
    println!("  parent_buckets:           {}", summary.parent_buckets);
    println!("  child_links:              {}", summary.child_links);
    println!("  logical_bytes:            {}", summary.logical_bytes);
    println!("  allocated_bytes:          {}", summary.allocated_bytes);
    println!("  elapsed:                  {:.3}s", elapsed.as_secs_f64());

    Ok(())
}
