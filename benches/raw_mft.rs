//! Divan benchmarks for the raw `$MFT` reader.
//!
//! Run on an elevated shell with:
//!
//! ```text
//! cargo bench --bench raw_mft
//! ```
//!
//! Set `USN_TEST_DRIVE` to choose the drive letter (default `C`). All
//! benches skip gracefully when the drive isn't NTFS or the process is
//! not elevated.

use std::env;
use std::num::NonZeroUsize;

use divan::Bencher;
use usn_journal_rs::{
    errors::UsnError, mft::Mft, path::PathResolver, raw_mft::RawMft, volume::Volume,
};

fn main() {
    divan::main();
}

/// Bound iteration so each bench sample finishes in a reasonable time
/// even on multi-million-record system drives.
const BENCH_RECORD_LIMIT: usize = 200_000;

fn pick_drive() -> char {
    env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

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

#[divan::bench]
fn raw_mft_iter(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };
    let mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    bencher.bench_local(|| {
        let mut count = 0u64;
        if let Ok(it) = mft.try_iter() {
            for r in it.take(BENCH_RECORD_LIMIT) {
                if r.is_ok() {
                    count += 1;
                }
            }
        }
        divan::black_box(count)
    });
}

#[divan::bench]
fn usn_mft_iter(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };
    let mft = Mft::new(&volume);
    bencher.bench_local(|| {
        let mut count = 0u64;
        if let Ok(it) = mft.try_iter() {
            for r in it.take(BENCH_RECORD_LIMIT) {
                if r.is_ok() {
                    count += 1;
                }
            }
        }
        divan::black_box(count)
    });
}

#[divan::bench]
fn raw_mft_iter_with_path_resolver(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };
    let mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    bencher.bench_local(|| {
        let mut resolver = PathResolver::builder(&volume).build();
        let mut count = 0u64;
        if let Ok(it) = mft.try_iter() {
            for r in it.flatten().take(BENCH_RECORD_LIMIT) {
                let _ = resolver.resolve_path(&r);
                count += 1;
            }
        }
        divan::black_box(count)
    });
}

#[divan::bench]
fn raw_mft_iter_with_cached_resolver(bencher: Bencher) {
    let Some(volume) = open_volume() else { return };
    let mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    bencher.bench_local(|| {
        let mut resolver = PathResolver::builder(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).expect("cache capacity must be non-zero"))
            .build();
        let mut count = 0u64;
        if let Ok(it) = mft.try_iter() {
            for r in it.flatten().take(BENCH_RECORD_LIMIT) {
                let _ = resolver.resolve_path(&r);
                count += 1;
            }
        }
        divan::black_box(count)
    });
}

/// Benchmark sweep over `buffer_bytes` sizes (in bytes).
/// The plan replaces `batch_records` with `buffer_bytes: NonZeroUsize`.
/// Parametrized in byte sizes: 16KB, 64KB, 256KB, 1MB.
#[divan::bench(args = [16 * 1024, 64 * 1024, 256 * 1024, 1024 * 1024])]
fn raw_mft_buffer_size(bencher: Bencher, buffer_bytes: usize) {
    let Some(volume) = open_volume() else { return };
    let mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    bencher.bench_local(|| {
        let opts = usn_journal_rs::raw_mft::RawMftIterOptions {
            buffer_bytes: std::num::NonZeroUsize::new(buffer_bytes).unwrap(),
            ..Default::default()
        };
        let mut count = 0u64;
        if let Ok(it) = mft.try_iter_with_options(opts) {
            for r in it.take(BENCH_RECORD_LIMIT) {
                if r.is_ok() {
                    count += 1;
                }
            }
        }
        divan::black_box(count)
    });
}
