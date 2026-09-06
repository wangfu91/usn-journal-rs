//! Criterion benchmarks for PathResolver performance across different strategies.
//!
//! This benchmark compares two live path resolution approaches:
//! 1. Pure syscall (no caching)
//! 2. Default syscall resolver with a directory cache
//!
//! Run on an elevated shell with:
//!
//! ```text
//! cargo bench --features windows-examples --bench path_resolver
//! ```
//!
//! Set `USN_TEST_DRIVE` to choose the drive letter (default `C`).
//! All benches skip gracefully when the drive isn't NTFS or the process
//! is not elevated.

use std::env;

use criterion::{Criterion, criterion_group, criterion_main};
use usn_journal_rs::{
    errors::UsnError,
    mft::MftEntry,
    path::PathResolver,
    volume::Volume,
};

/// Number of random entries to collect and resolve.
const NUM_TEST_ENTRIES: usize = 1000;

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

/// Collect entries from the Windows FSCTL enumerator.
fn collect_test_entries(volume: &Volume) -> Vec<MftEntry> {
    match volume.mft().try_iter() {
        Ok(iter) => iter.flatten().take(NUM_TEST_ENTRIES).collect(),
        Err(error) => {
            eprintln!("skipping: {error}");
            Vec::new()
        }
    }
}

/// Resolve paths with direct syscalls, no caching.
fn resolver_syscall_no_cache(c: &mut Criterion) {
    let Some(volume) = open_volume() else { return };
    let entries = collect_test_entries(&volume);

    if entries.is_empty() {
        eprintln!("skipping: no entries collected");
        return;
    }

    c.bench_function("resolver_syscall_no_cache", |b| {
        b.iter(|| {
            let resolver = PathResolver::new(&volume).with_directory_cache(0);
            let mut count = 0u64;

            for entry in &entries {
                let _ = resolver.resolve_path(entry);
                count += 1;
            }

            count
        })
    });
}

/// Resolve paths with a directory cache (8192 capacity).
fn resolver_syscall_directory_cache(c: &mut Criterion) {
    let Some(volume) = open_volume() else { return };
    let entries = collect_test_entries(&volume);

    if entries.is_empty() {
        eprintln!("skipping: no entries collected");
        return;
    }

    c.bench_function("resolver_syscall_directory_cache", |b| {
        b.iter(|| {
            let resolver = PathResolver::new(&volume).with_directory_cache(8192);

            // Warm-up pass to populate cache
            for entry in &entries {
                let _ = resolver.resolve_path(entry);
            }

            // Measured pass with warm cache
            let mut count = 0u64;
            for entry in &entries {
                let _ = resolver.resolve_path(entry);
                count += 1;
            }

            count
        })
    });
}

criterion_group!(
    path_resolver_benches,
    resolver_syscall_no_cache,
    resolver_syscall_directory_cache
);
criterion_main!(path_resolver_benches);
