use std::{error::Error, time::Instant};

use usn_journal_rs::raw_mft::{
    RawMft,
    ingest_support::{
        attr_list_profile_enabled, bench_config, open_volume, print_attr_list_profile,
        print_bench_config, print_scheduling_profile, run_parallel_ingest,
        run_parallel_ingest_with_profiles, scheduling_profile_enabled,
    },
};

fn main() -> Result<(), Box<dyn Error>> {
    let config = bench_config().clone();
    print_bench_config(&config);

    let Some(volume) = open_volume(config.drive) else {
        return Ok(());
    };
    let raw_mft = RawMft::new(&volume)?;

    let start = Instant::now();
    let collect_attr_list_profile = attr_list_profile_enabled();
    let collect_scheduling_profile = scheduling_profile_enabled();
    let (summary, profiles) = if collect_attr_list_profile || collect_scheduling_profile {
        run_parallel_ingest_with_profiles(
            &raw_mft,
            &config,
            collect_attr_list_profile,
            collect_scheduling_profile,
        )?
    } else {
        (run_parallel_ingest(&raw_mft, &config)?, Default::default())
    };
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
    if let Some(profile) = profiles.attr_list {
        print_attr_list_profile(&profile, elapsed);
    }
    if let Some(profile) = profiles.scheduling {
        print_scheduling_profile(&profile, elapsed);
    }

    Ok(())
}
