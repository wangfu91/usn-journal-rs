//! Integration test: `RawMft::path_resolver` resolves paths that exist on disk.
//!
//! Opens the NTFS C: volume, builds a `RawMft`, constructs its optimized
//! path resolver, iterates the first 200 used MFT records, resolves each
//! path, then verifies that >= 80 % of the resolved paths correspond to an
//! existing file or directory on disk (using the `\\?\` long-path prefix
//! required on Windows).
//!
//! Skips gracefully when the process is not elevated.

use usn_journal_rs::{
    errors::UsnError,
    raw_mft::{RawMft, RawMftEntry},
    volume::Volume,
};

#[test]
fn raw_mft_path_resolver_paths_exist_on_disk() {
    let volume = match Volume::from_drive_letter('C') {
        Ok(v) => v,
        Err(UsnError::NotElevated) => {
            eprintln!("raw_mft_path_resolver: skipping (requires admin privileges)");
            return;
        }
        Err(e) => {
            eprintln!("raw_mft_path_resolver: skipping: {e}");
            return;
        }
    };

    let raw_mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(UsnError::UnsupportedFilesystem(msg)) => {
            eprintln!("raw_mft_path_resolver: skipping (unsupported filesystem: {msg})");
            return;
        }
        Err(e) => panic!("RawMft::new failed unexpectedly: {e}"),
    };

    let resolver = raw_mft
        .path_resolver()
        .expect("RawMft::path_resolver should not fail on NTFS");

    let mut resolved = 0usize;
    let mut matched = 0usize;

    for entry in raw_mft
        .try_iter()
        .expect("RawMft::try_iter failed")
        .filter_map(|r: Result<RawMftEntry, UsnError>| r.ok())
        .filter(|e| e.is_used && !e.file_name.is_empty())
        .take(200)
    {
        let path = match resolver.resolve_path(&entry) {
            Some(p) => p,
            None => continue,
        };

        resolved += 1;

        // Prepend \\?\ for long-path support required on Windows.
        let long_path = format!(r"\\?\{}", path.display());
        if std::fs::metadata(&long_path).is_ok() {
            matched += 1;
        }
        // If metadata fails, the file was likely deleted between the MFT read
        // and now — that is expected and does not count as a failure.
    }

    assert!(
        resolved > 0,
        "expected at least some resolvable entries in the first 200 MFT records"
    );

    let match_rate = matched as f64 / resolved as f64;
    assert!(
        match_rate >= 0.80,
        "expected >= 80 % of resolved paths to exist on disk; \
         got {matched}/{resolved} ({:.1} %)",
        match_rate * 100.0
    );
}
