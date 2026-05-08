//! Criterion benchmark for the parallel raw-`$MFT` ingest path.
//!
//! Run with:
//!
//! ```text
//! cargo bench --bench raw_mft_ingest -- --sample-size 10
//! ```
//!
//! Save a baseline before an optimization wave with:
//!
//! ```text
//! cargo bench --bench raw_mft_ingest -- --sample-size 10 --save-baseline main
//! ```
//!
//! Compare a change against that baseline with:
//!
//! ```text
//! cargo bench --bench raw_mft_ingest -- --sample-size 10 --baseline main
//! ```
//!
//! Profile the exact same ingest workload with:
//!
//! ```text
//! cargo flamegraph -o raw_mft_parallel_ingest_profile.svg --example raw_mft_parallel_ingest_profile
//! ```
//!
//! For low-noise runs, keep worker count / chunk size fixed, close disk-heavy
//! background work, and benchmark the same drive state repeatedly.

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use usn_journal_rs::raw_mft::RawMft;

#[path = "support\\raw_mft_ingest_shared.rs"]
mod raw_mft_ingest_shared;

use raw_mft_ingest_shared::{
    bench_config, include_serial_bench, open_volume, print_bench_config, run_parallel_ingest,
    run_serial_ingest,
};

fn raw_mft_ingest_benchmarks(c: &mut Criterion) {
    let config = bench_config().clone();
    print_bench_config(&config);
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

    let mut group = c.benchmark_group("raw_mft_ingest");
    group.bench_function("parallel_ingest", |b| {
        b.iter(|| {
            let summary = match run_parallel_ingest(&mft, &config) {
                Ok(summary) => summary,
                Err(error) => {
                    eprintln!("parallel ingest bench failed: {error}");
                    raw_mft_ingest_shared::BenchSummary::default()
                }
            };
            std::hint::black_box(summary)
        });
    });

    if include_serial_bench() {
        group.bench_function("serial_ingest", |b| {
            b.iter(|| {
                let summary = match run_serial_ingest(&mft, &config) {
                    Ok(summary) => summary,
                    Err(error) => {
                        eprintln!("serial ingest bench failed: {error}");
                        raw_mft_ingest_shared::BenchSummary::default()
                    }
                };
                std::hint::black_box(summary)
            });
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(5))
        .measurement_time(Duration::from_secs(60))
        .configure_from_args();
    targets = raw_mft_ingest_benchmarks
}
criterion_main!(benches);
