//! Divan benchmarks for PathResolver performance across different strategies.
//!
//! This benchmark compares three path resolution approaches:
//! 1. Pure syscall (no caching)
//! 2. Default syscall resolver with a directory cache
//! 3. Raw-MFT-optimized resolver (one-time scan)
//!
//! Run on an elevated shell with:
//!
//! ```text
//! cargo bench --bench path_resolver
//! ```
//!
//! Set `USN_TEST_DRIVE` to choose the drive letter (default `C`).
//! All benches skip gracefully when the drive isn't NTFS or the process
//! is not elevated.

use std::env;

use divan::Bencher;
use usn_journal_rs::{
    errors::UsnError,
    mft::MftEntry,
    path::PathResolver,
    raw_mft::{RawMft, RawMftEntry},
    volume::Volume,
};

/// Run the Divan benchmark harness.
fn main() {
    divan::main();
}

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

/// Collect a list of MftEntry values from the $MFT up to NUM_TEST_ENTRIES.
fn collect_test_entries(volume: &Volume) -> Vec<MftEntry> {
    let mft = match RawMft::new(volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    if let Ok(it) = mft.try_iter() {
        for entry in it.flatten() {
            // Convert RawMftEntry to MftEntry for use with PathResolver
            let mft_entry = MftEntry {
                usn: usn_journal_rs::Usn::new(0),
                fid: entry.file_reference,
                parent_fid: entry.parent_reference,
                file_name: entry.file_name.clone(),
                file_attributes: usn_journal_rs::FileAttributes::empty(),
            };
            entries.push(mft_entry);
            if entries.len() >= NUM_TEST_ENTRIES {
                break;
            }
        }
    }
    entries
}

/// Collect a list of raw `$MFT` entries up to NUM_TEST_ENTRIES.
fn collect_raw_test_entries(volume: &Volume) -> Vec<RawMftEntry> {
    let mft = match RawMft::new(volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    if let Ok(it) = mft.try_iter() {
        for entry in it.flatten() {
            entries.push(entry);
            if entries.len() >= NUM_TEST_ENTRIES {
                break;
            }
        }
    }
    entries
}

/// Resolve paths with direct syscalls, no caching.
#[divan::bench]
fn resolver_syscall_no_cache(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };
    let entries = collect_test_entries(&volume);

    if entries.is_empty() {
        eprintln!("skipping: no entries collected");
        return;
    }

    bencher.bench_local(|| {
        let mut resolver = PathResolver::new(&volume).with_directory_cache(0);
        let mut count = 0u64;

        for entry in &entries {
            let _ = resolver.resolve_path(entry);
            count += 1;
        }

        divan::black_box(count)
    });
}

/// Resolve paths with a directory cache (8192 capacity).
#[divan::bench]
fn resolver_syscall_directory_cache(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };
    let entries = collect_test_entries(&volume);

    if entries.is_empty() {
        eprintln!("skipping: no entries collected");
        return;
    }

    bencher.bench_local(|| {
        let mut resolver = PathResolver::new(&volume).with_directory_cache(8192);

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

        divan::black_box(count)
    });
}

/// Resolve paths using the RawMft-optimized resolver (one-time full $MFT scan).
#[divan::bench]
fn resolver_raw_mft_optimized(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };

    let mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };

    let entries = collect_raw_test_entries(&volume);

    if entries.is_empty() {
        eprintln!("skipping: no entries collected");
        return;
    }

    bencher.bench_local(|| {
        let resolver = match mft.path_resolver() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipping: {e}");
                return divan::black_box(0u64);
            }
        };

        let mut count = 0u64;
        for entry in &entries {
            let _ = resolver.resolve_path(entry);
            count += 1;
        }

        divan::black_box(count)
    });
}
