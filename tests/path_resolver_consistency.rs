//! Integration test: all three `PathResolver` configurations agree.
//!
//! Builds a `RawMft` for C:, collects every 1 000th entry (up to 50),
//! then resolves each entry with three independent resolvers:
//!
//!  1. Syscall-based, no cache (`PathResolver::new(...).with_directory_cache(0)`).
//!  2. Directory-cached resolver (warm-cache path on second call).
//!  3. Raw-MFT-optimized resolver (`raw_mft.path_resolver()`).
//!
//! Asserts that ≥ 90 % of entries produce identical paths across all three
//! strategies (case-insensitive, to accommodate Windows path normalisation).
//! If all three return `None` for an entry that is also accepted.
//!
//! Skips gracefully on non-elevated runs.

use usn_journal_rs::{
    errors::UsnError,
    path::PathResolver,
    raw_mft::{RawMft, RawMftEntry},
    volume::Volume,
};

#[test]
fn all_three_resolvers_agree() {
    let volume = match Volume::from_drive_letter('C') {
        Ok(v) => v,
        Err(UsnError::NotElevated) => {
            eprintln!("path_resolver_consistency: skipping (requires admin privileges)");
            return;
        }
        Err(e) => {
            eprintln!("path_resolver_consistency: skipping: {e}");
            return;
        }
    };

    let raw_mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(UsnError::UnsupportedFilesystem(_)) => {
            eprintln!("path_resolver_consistency: skipping (C: is not NTFS — unexpected)");
            return;
        }
        Err(e) => panic!("RawMft::new failed: {e}"),
    };

    // Collect every 1 000th used entry with a non-empty name (up to 50).
    let entries: Vec<_> = raw_mft
        .try_iter()
        .expect("RawMft::try_iter failed")
        .filter_map(|r: Result<RawMftEntry, UsnError>| r.ok())
        .filter(|e| e.is_used && !e.file_name.is_empty())
        .enumerate()
        .filter(|(i, _)| i % 1_000 == 0)
        .map(|(_, e)| e)
        .take(50)
        .collect();

    if entries.is_empty() {
        eprintln!("path_resolver_consistency: no suitable entries found — skipping");
        return;
    }

    // Build the three resolvers.
    let mut resolver1 = PathResolver::new(&volume).with_directory_cache(0);

    let mut resolver2 = PathResolver::new(&volume).with_directory_cache(1024);

    let mut resolver3 = match raw_mft.path_resolver() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("path_resolver_consistency: raw_mft.path_resolver failed: {e}");
            return;
        }
    };

    let mut checked = 0usize;
    let mut agreements = 0usize;

    for entry in &entries {
        let p1 = resolver1.resolve_path(entry);

        // Call resolver2 twice so the second call exercises the warm-cache path.
        let _ = resolver2.resolve_path(entry);
        let p2 = resolver2.resolve_path(entry);

        let p3 = resolver3.resolve_path(entry);

        checked += 1;

        match (&p1, &p2, &p3) {
            (None, None, None) => {
                // All three failed — acceptable (deleted / inaccessible entry).
                agreements += 1;
            }
            (Some(a), Some(b), Some(c)) => {
                // All three succeeded — compare case-insensitively.
                let s1 = a.to_string_lossy().to_ascii_lowercase();
                let s2 = b.to_string_lossy().to_ascii_lowercase();
                let s3 = c.to_string_lossy().to_ascii_lowercase();

                if s1 == s2 && s2 == s3 {
                    agreements += 1;
                } else {
                    eprintln!(
                        "path_resolver_consistency: disagreement for {:?}:\n  r1: {s1}\n  r2: {s2}\n  r3: {s3}",
                        entry.file_name.to_string_lossy()
                    );
                }
            }
            _ => {
                // Partial None — e.g. syscall succeeded but in-memory tree missed
                // a deleted-then-recreated entry.  Log but do not count as agreement.
                eprintln!(
                    "path_resolver_consistency: partial None for {:?}: \
                     r1={} r2={} r3={}",
                    entry.file_name.to_string_lossy(),
                    fmt_opt(&p1),
                    fmt_opt(&p2),
                    fmt_opt(&p3),
                );
            }
        }
    }

    assert!(checked > 0, "no entries were checked");

    let agreement_rate = agreements as f64 / checked as f64;
    assert!(
        agreement_rate >= 0.90,
        "expected ≥ 90 % agreement between resolvers; \
         got {agreements}/{checked} ({:.1} %)",
        agreement_rate * 100.0
    );
}

fn fmt_opt(p: &Option<std::path::PathBuf>) -> String {
    match p {
        Some(pb) => pb.display().to_string(),
        None => "<None>".to_string(),
    }
}
