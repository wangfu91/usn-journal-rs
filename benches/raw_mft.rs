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

use divan::Bencher;
use usn_journal_rs::{
    errors::UsnError, mft::Mft, path::PathResolver, raw_mft::RawMft, volume::Volume,
};

fn main() {
    divan::main();
}

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
        Err(UsnError::PermissionError) => {
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
        if let Ok(it) = mft.iter() {
            for r in it {
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
        if let Ok(it) = mft.iter() {
            for r in it {
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
        let mut resolver = PathResolver::new(&volume);
        let mut count = 0u64;
        if let Ok(it) = mft.iter() {
            for r in it.flatten() {
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
        let mut resolver = PathResolver::new_with_cache(&volume);
        let mut count = 0u64;
        if let Ok(it) = mft.iter() {
            for r in it.flatten() {
                let _ = resolver.resolve_path(&r);
                count += 1;
            }
        }
        divan::black_box(count)
    });
}

#[divan::bench(args = [16usize, 64, 256, 1024])]
fn raw_mft_batch_size(bencher: Bencher, batch: usize) {
    let Some(volume) = open_volume() else { return };
    let mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    bencher.bench_local(|| {
        let mut opts = usn_journal_rs::raw_mft::RawMftOptions::default();
        opts.batch_records = batch;
        let mut count = 0u64;
        if let Ok(it) = mft.iter_with_options(opts) {
            for r in it {
                if r.is_ok() {
                    count += 1;
                }
            }
        }
        divan::black_box(count)
    });
}
