//! Criterion benchmark for the serial raw-`$MFT` read path.
//!
//! Run on an elevated shell with:
//!
//! ```text
//! cargo bench --bench raw_mft -- --sample-size 20
//! ```
//!
//! Save a baseline before an optimization wave with:
//!
//! ```text
//! cargo bench --bench raw_mft -- --sample-size 20 --save-baseline serial-start
//! ```
//!
//! Compare a change against that baseline with:
//!
//! ```text
//! cargo bench --bench raw_mft -- --sample-size 20 --baseline serial-start
//! ```
//!
//! Profile the exact same workload with:
//!
//! ```text
//! cargo flamegraph -o raw_mft_serial_read.svg --example raw_mft_serial_read_profile
//! ```
//!
//! On Windows, set `USN_TEST_DRIVE` to choose the drive letter (default `C`).
//! On Linux, set `USN_TEST_VOLUME` to an NTFS mount or device path. All benches
//! skip gracefully when the source isn't NTFS or cannot be read.
//! Set `USN_RAW_MFT_SERIAL_MAX_RECORDS` to cap the per-run record count for
//! faster optimization-loop iterations.

use std::{num::NonZeroUsize, time::Duration};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use usn_journal_rs::raw_mft::{RawMft, RawMftScanOptions};

#[path = "../support/raw_mft_serial_support.rs"]
mod serial_support;

use serial_support::{
    SerialConfig, SerialSummary, nonzero_usize, open_volume, print_serial_config, run_serial_read,
};

/// Main-buffer sizes (in bytes) swept by the optional buffer-size group.
const BUFFER_SWEEP_BYTES: [usize; 4] = [64 * 1024, 256 * 1024, 1024 * 1024, 4 * 1024 * 1024];

fn raw_mft_serial_benchmarks(c: &mut Criterion) {
    let config = SerialConfig::from_env();
    print_serial_config(&config);

    let Some(volume) = open_volume(config.drive) else {
        return;
    };
    let mft = match RawMft::new(&volume) {
        Ok(mft) => mft,
        Err(error) => {
            eprintln!("skipping bench: {error}");
            return;
        }
    };

    // Warm the OS cache and report the workload shape so the measured run
    // reflects parser CPU rather than first-touch cold reads.
    match run_serial_read(&mft, &config) {
        Ok(summary) => print_summary(&summary),
        Err(error) => {
            eprintln!("skipping bench: {error}");
            return;
        }
    }

    let mut group = c.benchmark_group("raw_mft_serial");
    group.bench_function("read", |b| {
        b.iter(|| {
            let summary = run_serial_read(&mft, &config).unwrap_or_default();
            std::hint::black_box(summary)
        });
    });
    group.finish();

    if buffer_sweep_enabled() {
        let mut sweep = c.benchmark_group("raw_mft_serial_buffer");
        for bytes in BUFFER_SWEEP_BYTES {
            let buffer_bytes = nonzero_usize(bytes);
            let bench_id = BenchmarkId::new("read", bytes);
            sweep.bench_with_input(bench_id, &buffer_bytes, |b, &buffer_bytes| {
                b.iter(|| {
                    let summary = run_with_buffer(&mft, &config, buffer_bytes).unwrap_or_default();
                    std::hint::black_box(summary)
                });
            });
        }
        sweep.finish();
    }
}

/// Run the serial read with an overridden main-buffer size.
fn run_with_buffer(
    mft: &RawMft<'_>,
    config: &SerialConfig,
    buffer_bytes: NonZeroUsize,
) -> Result<SerialSummary, usn_journal_rs::errors::UsnError> {
    let options: RawMftScanOptions = RawMftScanOptions::builder()
        .buffer_bytes(buffer_bytes)
        .include_unused_records(config.include_unused_records)
        .collect_alternate_data_streams(config.collect_alternate_data_streams)
        .collect_data_run_summary(config.collect_data_run_summary)
        .collect_dos_file_name_links(config.collect_dos_file_name_links)
        .start_record(config.start_record)
        .end_record(config.end_record)
        .build();

    let mut summary = SerialSummary::default();
    let limit = config.max_records.unwrap_or(usize::MAX);
    for item in mft.try_iter_with_options(options)?.take(limit) {
        match item {
            Ok(entry) => {
                summary.records += 1;
                summary.name_chars += entry.file_name.len() as u64;
                summary.links += entry.links.len() as u64;
                summary.ads += entry.alternate_data_streams.len() as u64;
                summary.size_checksum ^= entry.real_size ^ entry.allocated_size;
            }
            Err(_) => summary.errors += 1,
        }
    }
    Ok(summary)
}

/// Whether the optional buffer-size sweep group should run.
fn buffer_sweep_enabled() -> bool {
    std::env::var_os("USN_RAW_MFT_SERIAL_BUFFER_SWEEP").is_some()
}

/// Print the warm-up workload summary so each run records its shape.
fn print_summary(summary: &SerialSummary) {
    eprintln!(
        "raw_mft serial workload: records={} errors={} name_chars={} links={} ads={}",
        summary.records, summary.errors, summary.name_chars, summary.links, summary.ads,
    );
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(3))
        .measurement_time(Duration::from_secs(30))
        .configure_from_args();
    targets = raw_mft_serial_benchmarks
}
criterion_main!(benches);
