//! Criterion benchmarks for USN journal iteration.
//!
//! Run on an elevated shell with:
//!
//! ```text
//! cargo bench --bench journal
//! ```
//!
//! Set `USN_TEST_DRIVE` to choose the drive letter (default `C`). Set
//! `BENCH_RECORD_LIMIT` to limit iterations per bench (default 100_000).
//! All benches skip gracefully when the drive isn't NTFS/ReFS or the process
//! is not elevated.

use std::env;

use criterion::{Criterion, criterion_group, criterion_main};
use usn_journal_rs::{errors::UsnError, journal::UsnJournal, volume::Volume};

/// Bound iteration so each bench sample finishes in a reasonable time
/// even on large journals.
fn bench_record_limit() -> usize {
    env::var("BENCH_RECORD_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000)
}

/// Read the drive letter to benchmark from `USN_TEST_DRIVE`.
fn pick_drive() -> char {
    env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

/// Open the benchmark target volume or skip when the environment is unsuitable.
fn open_volume() -> Option<Volume> {
    match Volume::from_drive_letter(pick_drive()) {
        Ok(v) => Some(v),
        Err(UsnError::NotElevated) => {
            eprintln!("skipping bench: requires admin privileges");
            None
        }
        Err(e) => {
            eprintln!("skipping bench: {e}");
            None
        }
    }
}

/// Iterate the full USN journal with all reason flags enabled.
fn journal_iter_full_mask(c: &mut Criterion) {
    let Some(volume) = open_volume() else { return };
    let limit = bench_record_limit();

    c.bench_function("journal_iter_full_mask", |b| {
        b.iter(|| {
            let journal = UsnJournal::new(&volume);
            let mut count = 0u64;
            if let Ok(it) = journal.try_iter() {
                for r in it.take(limit) {
                    if r.is_ok() {
                        count += 1;
                    }
                }
            }
            count
        })
    });
}

/// Iterate the USN journal with a restricted reason mask
/// (FILE_CREATE | FILE_DELETE).
fn journal_iter_filtered(c: &mut Criterion) {
    let Some(volume) = open_volume() else { return };
    let limit = bench_record_limit();

    use usn_journal_rs::UsnReason;
    use windows::Win32::System::Ioctl::{USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE};

    c.bench_function("journal_iter_filtered", |b| {
        b.iter(|| {
            let journal = UsnJournal::new(&volume);
            let opts = usn_journal_rs::journal::JournalIterOptions::builder()
                .reason_mask(UsnReason::from_bits_retain(
                    USN_REASON_FILE_CREATE | USN_REASON_FILE_DELETE,
                ))
                .build();
            let mut count = 0u64;
            if let Ok(it) = journal.try_iter_with_options(opts) {
                for r in it.take(limit) {
                    if r.is_ok() {
                        count += 1;
                    }
                }
            }
            count
        })
    });
}

criterion_group!(
    journal_benches,
    journal_iter_full_mask,
    journal_iter_filtered
);
criterion_main!(journal_benches);
