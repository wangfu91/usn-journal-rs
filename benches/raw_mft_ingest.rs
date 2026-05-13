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
use usn_journal_rs::raw_mft::{
    RawMft,
    ingest_support::{
        self, BenchScheduling, bench_config, include_serial_bench, open_volume, print_bench_config,
        print_summary_enabled, run_parallel_ingest, run_serial_ingest, scheduling_sweep_values,
        summary_run_count, worker_sweep_values, workload_shape,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SummaryCase {
    workers: NonZeroUsize,
    scheduling: BenchScheduling,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SummaryRow {
    elapsed: Duration,
    top_hit_rate: Option<f64>,
    top_half_hits: Option<(usize, usize)>,
    top_quarter_hits: Option<(usize, usize)>,
    missed_actual: Option<usize>,
    worst_pred_rank: Option<usize>,
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
        BenchScheduling::DynamicPhysicalOrder => "dynamic-physical-order",
        BenchScheduling::DynamicCostBanded => "dynamic-cost-banded",
        BenchScheduling::DynamicObservedAdaptive => "dynamic-observed-adaptive",
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
    eprintln!("| workers | scheduling | elapsed | top-hit | top-half | top-quarter | missed | worst-pred-rank |");
    eprintln!("| ------: | ---------- | ------: | -------: | -------: | ----------: | -----: | ---------------: |");

    for case in cases {
        let case_config = config
            .with_worker_count(case.workers)
            .with_scheduling(case.scheduling);
        match median_summary_row(run_count, || summary_row_for_case(mft, &case_config, case.scheduling)) {
            Ok(row) => {
                eprintln!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} |",
                    case.workers,
                    scheduling_label(case.scheduling),
                    format_duration(row.elapsed),
                    format_percent_option(row.top_hit_rate),
                    format_count_pair_option(row.top_half_hits),
                    format_count_pair_option(row.top_quarter_hits),
                    format_usize_option(row.missed_actual),
                    format_usize_option(row.worst_pred_rank),
                );
            }
            Err(error) => {
                eprintln!(
                    "| {} | {} | error: {} | - | - | - | - | - |",
                    case.workers,
                    scheduling_label(case.scheduling),
                    error,
                );
            }
        }
    }
}

fn summary_row_for_case(
    mft: &RawMft<'_>,
    config: &ingest_support::BenchConfig,
    scheduling: BenchScheduling,
) -> Result<SummaryRow, usn_journal_rs::errors::UsnError> {
    let start = Instant::now();
    let scheduling_profile = if matches!(scheduling, BenchScheduling::DynamicObservedAdaptive) {
        let (_, profiles) = ingest_support::run_parallel_ingest_with_profiles(mft, config, false, true)?;
        profiles.scheduling
    } else {
        let summary = run_parallel_ingest(mft, config)?;
        std::hint::black_box(summary);
        None
    };

    Ok(SummaryRow {
        elapsed: start.elapsed(),
        top_hit_rate: scheduling_profile.as_ref().map(|profile| profile.actual_top_hit_rate()),
        top_half_hits: scheduling_profile
            .as_ref()
            .map(|profile| (profile.actual_top_in_predicted_top_half, profile.compared_top_k)),
        top_quarter_hits: scheduling_profile
            .as_ref()
            .map(|profile| (profile.actual_top_in_predicted_top_quarter, profile.compared_top_k)),
        missed_actual: scheduling_profile
            .as_ref()
            .map(|profile| profile.actual_top_missed_by_predicted_top_k),
        worst_pred_rank: scheduling_profile
            .as_ref()
            .map(|profile| profile.actual_top_worst_predicted_rank),
    })
}

fn summary_cases(config: &ingest_support::BenchConfig) -> Vec<SummaryCase> {
    let mut cases = Vec::new();
    push_unique_case(
        &mut cases,
        SummaryCase {
            workers: config.worker_count,
            scheduling: config.scheduling_mode(),
        },
    );

    for worker_count in worker_sweep_values() {
        push_unique_case(
            &mut cases,
            SummaryCase {
                workers: worker_count,
                scheduling: config.scheduling_mode(),
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

fn median_summary_row<F>(
    run_count: NonZeroUsize,
    mut f: F,
) -> Result<SummaryRow, usn_journal_rs::errors::UsnError>
where
    F: FnMut() -> Result<SummaryRow, usn_journal_rs::errors::UsnError>,
{
    let mut rows = Vec::with_capacity(run_count.get());
    for _ in 0..run_count.get() {
        rows.push(f()?);
    }
    rows.sort_unstable_by(|left, right| left.elapsed.cmp(&right.elapsed));
    Ok(rows[rows.len() / 2])
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.3} s", duration.as_secs_f64())
    } else {
        format!("{:.1} ms", duration.as_secs_f64() * 1_000.0)
    }
}

fn format_percent_option(value: Option<f64>) -> String {
    value
        .map(|value| format!("{:.2}%", value * 100.0))
        .unwrap_or_else(|| "-".to_owned())
}

fn format_count_pair_option(value: Option<(usize, usize)>) -> String {
    value
        .map(|(hits, total)| format!("{hits}/{total}"))
        .unwrap_or_else(|| "-".to_owned())
}

fn format_usize_option(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_owned())
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
