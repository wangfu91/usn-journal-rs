//! Integration test: `InMemoryDirTree` resolves paths that exist on disk.
//!
//! Opens the NTFS C: volume, builds an `InMemoryDirTree`, iterates the first
//! 200 used MFT records, resolves each path, then verifies that ≥ 80 % of the
//! resolved paths correspond to an existing file or directory on disk (using
//! the `\\?\` long-path prefix required on Windows).
//!
//! Skips gracefully when the process is not elevated.

use usn_journal_rs::{
    errors::UsnError,
    path::InMemoryDirTree,
    raw_mft::{RawMft, RawMftEntry},
    volume::Volume,
};

#[test]
fn in_memory_tree_paths_exist_on_disk() {
    let volume = match Volume::from_drive_letter('C') {
        Ok(v) => v,
        Err(UsnError::NotElevated) => {
            eprintln!("in_memory_tree: skipping (requires admin privileges)");
            return;
        }
        Err(e) => {
            eprintln!("in_memory_tree: skipping: {e}");
            return;
        }
    };

    let raw_mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(UsnError::UnsupportedFilesystem(msg)) => {
            eprintln!("in_memory_tree: skipping (unsupported filesystem: {msg})");
            return;
        }
        Err(e) => panic!("RawMft::new failed unexpectedly: {e}"),
    };

    let tree = InMemoryDirTree::from_raw_mft(&raw_mft)
        .expect("InMemoryDirTree::from_raw_mft should not fail on NTFS");

    assert!(
        !tree.is_empty(),
        "directory tree must contain at least one entry"
    );

    let mut resolved = 0usize;
    let mut matched = 0usize;

    for entry in raw_mft
        .iter()
        .expect("RawMft::iter failed")
        .filter_map(|r: Result<RawMftEntry, UsnError>| r.ok())
        .filter(|e| e.is_used && !e.file_name.is_empty())
        .take(200)
    {
        let path = match tree.resolve_with_drive_letter(entry.file_reference, 'C') {
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
        "expected ≥ 80 % of resolved paths to exist on disk; \
         got {matched}/{resolved} ({:.1} %)",
        match_rate * 100.0
    );
}
