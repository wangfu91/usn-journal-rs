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

use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use usn_journal_rs::raw_mft::RawMft;

#[path = "../support/raw_mft_ingest_support.rs"]
mod ingest_support;

use ingest_support::{
    BenchScheduling, bench_config, include_serial_bench, open_volume, print_bench_config,
    print_summary_enabled, run_parallel_ingest, run_serial_ingest, scheduling_sweep_values,
    summary_run_count, worker_sweep_values, workload_shape,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SummaryCase {
    workers: NonZeroUsize,
    scheduling: BenchScheduling,
}

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
    let shape = workload_shape(&mft, &config);
    eprintln!(
        "raw_mft_ingest workload: record_count={} planned_chunks={} file_record_size={} cluster_size={}",
        shape.record_count, shape.planned_chunks, shape.file_record_size, shape.cluster_size,
    );
    maybe_print_summary_table(&mft, &config);

    let mut baseline_group = c.benchmark_group("raw_mft_ingest");
    baseline_group.bench_function("parallel_ingest", |b| {
        b.iter(|| {
            let summary = match run_parallel_ingest(&mft, &config) {
                Ok(summary) => summary,
                Err(error) => {
                    eprintln!("parallel ingest bench failed: {error}");
                    ingest_support::BenchSummary::default()
                }
            };
            std::hint::black_box(summary)
        });
    });

    if include_serial_bench() {
        baseline_group.bench_function("serial_ingest", |b| {
            b.iter(|| {
                let summary = match run_serial_ingest(&mft, &config) {
                    Ok(summary) => summary,
                    Err(error) => {
                        eprintln!("serial ingest bench failed: {error}");
                        ingest_support::BenchSummary::default()
                    }
                };
                std::hint::black_box(summary)
            });
        });
    }
    baseline_group.finish();

    let worker_sweep = worker_sweep_values();
    if !worker_sweep.is_empty() {
        let mut worker_group = c.benchmark_group("raw_mft_ingest_workers");
        for worker_count in worker_sweep {
            let sweep_config = config.with_worker_count(worker_count);
            let bench_id = BenchmarkId::new("parallel_ingest", worker_count.get());
            worker_group.bench_with_input(bench_id, &sweep_config, |b, sweep_config| {
                b.iter(|| {
                    let summary = match run_parallel_ingest(&mft, sweep_config) {
                        Ok(summary) => summary,
                        Err(error) => {
                            eprintln!("parallel ingest worker sweep failed: {error}");
                            ingest_support::BenchSummary::default()
                        }
                    };
                    std::hint::black_box(summary)
                });
            });
        }
        worker_group.finish();
    }

    let scheduling_sweep = scheduling_sweep_values();
    if !scheduling_sweep.is_empty() {
        let mut scheduling_group = c.benchmark_group("raw_mft_ingest_scheduling");
        for scheduling in scheduling_sweep {
            let sweep_config = config.with_scheduling(scheduling);
            let bench_id = BenchmarkId::new("parallel_ingest", scheduling_label(scheduling));
            scheduling_group.bench_with_input(bench_id, &sweep_config, |b, sweep_config| {
                b.iter(|| {
                    let summary = match run_parallel_ingest(&mft, sweep_config) {
                        Ok(summary) => summary,
                        Err(error) => {
                            eprintln!("parallel ingest scheduling sweep failed: {error}");
                            ingest_support::BenchSummary::default()
                        }
                    };
                    std::hint::black_box(summary)
                });
            });
        }
        scheduling_group.finish();
    }
}

fn scheduling_label(scheduling: BenchScheduling) -> &'static str {
    match scheduling {
        BenchScheduling::Dynamic => "dynamic",
        BenchScheduling::Contiguous => "contiguous",
    }
}

fn maybe_print_summary_table(mft: &RawMft<'_>, config: &ingest_support::BenchConfig) {
    if !print_summary_enabled() {
        return;
    }

    let run_count = summary_run_count();
    let cases = summary_cases(config);
    eprintln!(
        "raw_mft_ingest one-shot summary (median of {} run(s); wall-clock only, not Criterion confidence intervals):",
        run_count.get()
    );
    eprintln!("| workers | scheduling | elapsed |");
    eprintln!("| ------: | ---------- | ------: |");

    for case in cases {
        let case_config = config
            .with_worker_count(case.workers)
            .with_scheduling(case.scheduling);
        match median_elapsed(run_count, || run_parallel_ingest(mft, &case_config)) {
            Ok(elapsed) => {
                eprintln!(
                    "| {} | {} | {} |",
                    case.workers,
                    scheduling_label(case.scheduling),
                    format_duration(elapsed),
                );
            }
            Err(error) => {
                eprintln!(
                    "| {} | {} | error: {} |",
                    case.workers,
                    scheduling_label(case.scheduling),
                    error,
                );
            }
        }
    }
}

fn summary_cases(config: &ingest_support::BenchConfig) -> Vec<SummaryCase> {
    let mut cases = Vec::new();
    push_unique_case(
        &mut cases,
        SummaryCase {
            workers: config.worker_count,
            scheduling: match config.scheduling_label() {
                "contiguous" => BenchScheduling::Contiguous,
                _ => BenchScheduling::Dynamic,
            },
        },
    );

    for worker_count in worker_sweep_values() {
        push_unique_case(
            &mut cases,
            SummaryCase {
                workers: worker_count,
                scheduling: match config.scheduling_label() {
                    "contiguous" => BenchScheduling::Contiguous,
                    _ => BenchScheduling::Dynamic,
                },
            },
        );
    }

    for scheduling in scheduling_sweep_values() {
        push_unique_case(
            &mut cases,
            SummaryCase {
                workers: config.worker_count,
                scheduling,
            },
        );
    }

    cases
}

fn push_unique_case(cases: &mut Vec<SummaryCase>, case: SummaryCase) {
    if !cases.contains(&case) {
        cases.push(case);
    }
}

fn median_elapsed<F>(
    run_count: NonZeroUsize,
    mut f: F,
) -> Result<Duration, usn_journal_rs::errors::UsnError>
where
    F: FnMut() -> Result<ingest_support::BenchSummary, usn_journal_rs::errors::UsnError>,
{
    let mut durations = Vec::with_capacity(run_count.get());
    for _ in 0..run_count.get() {
        let start = Instant::now();
        let summary = f()?;
        std::hint::black_box(summary);
        durations.push(start.elapsed());
    }
    durations.sort_unstable();
    Ok(durations[durations.len() / 2])
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.3} s", duration.as_secs_f64())
    } else {
        format!("{:.1} ms", duration.as_secs_f64() * 1_000.0)
    }
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
