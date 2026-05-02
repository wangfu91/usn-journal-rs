//! Integration test: compare `RawMft` records with `Mft` API records.
//!
//! We sample entries from `Mft` (`FSCTL_ENUM_USN_DATA`), then look up the
//! corresponding raw FILE record by record number via `RawMft::get_record`.
//! The test asserts high agreement on core identity fields:
//!   - file ID (`fid` / `file_reference`)
//!   - parent file ID
//!   - directory bit
//!   - file name (case-insensitive)
//!
//! The filesystem is live, so a small mismatch rate is tolerated.

use std::collections::HashSet;

use usn_journal_rs::{
    errors::UsnError, mft::Mft, path::PathResolver, raw_mft::RawMft, volume::Volume,
};

struct SampledMftEntry {
    fid: usn_journal_rs::Fid,
    parent_fid: usn_journal_rs::Fid,
    is_dir: bool,
    file_name: std::ffi::OsString,
}

fn pick_drive() -> char {
    std::env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

#[test]
fn raw_mft_and_mft_api_agree_on_sampled_entries() {
    let drive = pick_drive();
    let volume = match Volume::from_drive_letter(drive) {
        Ok(v) => v,
        Err(UsnError::NotElevated) => {
            eprintln!("raw_mft_mft_consistency: skipping (requires admin privileges)");
            return;
        }
        Err(e) => {
            eprintln!("raw_mft_mft_consistency: skipping on {drive}: {e}");
            return;
        }
    };

    let mft = Mft::new(&volume);
    let mut iter = match mft.try_iter() {
        Ok(it) => it,
        Err(e) => {
            eprintln!("raw_mft_mft_consistency: skipping (Mft::try_iter failed: {e})");
            return;
        }
    };

    const SAMPLE_LIMIT: usize = 10_000;
    const MAX_MFT_SCAN: usize = 250_000;
    const MIN_COMPARED: usize = 200;
    const PATH_TARGET: usize = 2_000;

    let mut mft_path_resolver = PathResolver::builder(&volume).build();
    let mut mft_paths = HashSet::with_capacity(PATH_TARGET);

    let mut samples = Vec::with_capacity(SAMPLE_LIMIT);
    let mut mft_ok = 0usize;
    let mut mft_err = 0usize;
    let mut scanned = 0usize;

    while (samples.len() < SAMPLE_LIMIT || mft_paths.len() < PATH_TARGET) && scanned < MAX_MFT_SCAN
    {
        let Some(item) = iter.next() else {
            break;
        };
        scanned += 1;

        match item {
            Ok(entry) => {
                mft_ok += 1;
                if !entry.file_name.is_empty() && mft_paths.len() < PATH_TARGET {
                    if let Some(path) = mft_path_resolver.resolve_path(&entry) {
                        mft_paths.insert(path.to_string_lossy().to_ascii_lowercase());
                    }
                }
                if entry.fid.is_standard() && !entry.file_name.is_empty() {
                    samples.push(SampledMftEntry {
                        fid: entry.fid,
                        parent_fid: entry.parent_fid,
                        is_dir: entry.is_dir(),
                        file_name: entry.file_name,
                    });
                }
            }
            Err(_) => {
                // Keep scanning: individual record parse errors are expected to be non-fatal.
                mft_err += 1;
            }
        }
    }

    if samples.is_empty() && mft_paths.is_empty() {
        eprintln!(
            "raw_mft_mft_consistency: skipping (no comparable Mft entries, ok={mft_ok}, err={mft_err}, scanned={scanned})"
        );
        return;
    }

    let raw_mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(UsnError::UnsupportedFilesystem(msg)) => {
            eprintln!(
                "raw_mft_mft_consistency: skipping (unsupported filesystem on {drive}: {msg})"
            );
            return;
        }
        Err(e) => panic!("RawMft::new failed: {e}"),
    };

    if !samples.is_empty() {
        let mut compared = 0usize;
        let mut agreements = 0usize;
        let mut lookup_misses = 0usize;
        let mut logged_disagreements = 0usize;

        for entry in &samples {
            let Some(record_number) = entry.fid.record_number() else {
                continue;
            };

            let raw_entry = match raw_mft.get_record(record_number) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    lookup_misses += 1;
                    continue;
                }
                Err(e) => panic!(
                    "RawMft::get_record({record_number}) failed for fid {}: {e}",
                    entry.fid
                ),
            };

            if !raw_entry.is_used {
                lookup_misses += 1;
                continue;
            }

            compared += 1;

            let same_fid = raw_entry.file_reference == entry.fid;
            let same_parent = raw_entry.parent_reference == entry.parent_fid;
            let same_dir = raw_entry.is_directory == entry.is_dir;
            let same_name = raw_entry
                .file_name
                .to_string_lossy()
                .eq_ignore_ascii_case(&entry.file_name.to_string_lossy());

            if same_fid && same_parent && same_dir && same_name {
                agreements += 1;
            } else if logged_disagreements < 10 {
                logged_disagreements += 1;
                eprintln!(
                    "raw_mft_mft_consistency: mismatch fid={} record={} name(raw='{}', mft='{}') parent(raw={}, mft={}) dir(raw={}, mft={})",
                    entry.fid,
                    record_number,
                    raw_entry.file_name.to_string_lossy(),
                    entry.file_name.to_string_lossy(),
                    raw_entry.parent_reference,
                    entry.parent_fid,
                    raw_entry.is_directory,
                    entry.is_dir,
                );
            }
        }

        assert!(
            compared >= MIN_COMPARED,
            "expected to compare at least {MIN_COMPARED} entries; got {compared} (lookup_misses={lookup_misses})"
        );

        let agreement_rate = agreements as f64 / compared as f64;
        assert!(
            agreement_rate >= 0.90,
            "expected >= 90% agreement between RawMft and Mft API; got {agreements}/{compared} ({:.1}%), lookup_misses={lookup_misses}",
            agreement_rate * 100.0
        );
        return;
    }

    // Fallback for environments that only surface extended MFT IDs.
    let mut raw_paths = HashSet::with_capacity(PATH_TARGET);
    let mut raw_path_resolver = PathResolver::builder(&volume).build();
    for raw_entry in raw_mft
        .try_iter()
        .expect("RawMft::try_iter failed")
        .flatten()
    {
        if !raw_entry.is_used || raw_entry.file_name.is_empty() {
            continue;
        }
        if let Some(path) = raw_path_resolver.resolve_path(&raw_entry) {
            raw_paths.insert(path.to_string_lossy().to_ascii_lowercase());
        }
        if raw_paths.len() >= PATH_TARGET {
            break;
        }
    }

    if mft_paths.is_empty() || raw_paths.is_empty() {
        eprintln!(
            "raw_mft_mft_consistency: skipping path-overlap fallback (mft_paths={}, raw_paths={})",
            mft_paths.len(),
            raw_paths.len()
        );
        return;
    }

    let overlap = mft_paths.intersection(&raw_paths).count();
    let mft_overlap_rate = overlap as f64 / mft_paths.len() as f64;
    let raw_overlap_rate = overlap as f64 / raw_paths.len() as f64;

    assert!(
        mft_overlap_rate >= 0.70,
        "expected >= 70% of sampled Mft paths to exist in sampled RawMft paths; got {overlap}/{} ({:.1}%)",
        mft_paths.len(),
        mft_overlap_rate * 100.0
    );
    assert!(
        raw_overlap_rate >= 0.70,
        "expected >= 70% of sampled RawMft paths to exist in sampled Mft paths; got {overlap}/{} ({:.1}%)",
        raw_paths.len(),
        raw_overlap_rate * 100.0
    );
}

/// Strict record-by-record parity check using standard NTFS file IDs.
///
/// For each `Mft` entry that carries a standard 64-bit `Fid`, the matching
/// raw FILE record is fetched by record number with [`RawMft::get_record`] and
/// compared field-by-field.  Because the filesystem is live (log files are
/// created and deleted continuously), a few mismatches between the two reads
/// are normal and tolerated.
///
/// Skips automatically when:
///  - the process is not elevated,
///  - `C:` (or the drive set by `USN_TEST_DRIVE`) is not NTFS, or
///  - the `Mft` iterator yields only extended IDs (e.g. on ReFS or certain
///    Windows 11 builds with ReFS-style IDs on NTFS).
#[test]
fn raw_mft_and_mft_api_record_parity_standard_ids() {
    let drive = pick_drive();
    let volume = match Volume::from_drive_letter(drive) {
        Ok(v) => v,
        Err(UsnError::NotElevated) => {
            eprintln!("raw_mft_record_parity: skipping (requires admin privileges)");
            return;
        }
        Err(e) => {
            eprintln!("raw_mft_record_parity: skipping on {drive}: {e}");
            return;
        }
    };

    // Collect up to SAMPLE_LIMIT Mft entries that carry a standard (64-bit) FID.
    // Scan at most MAX_SCAN records before giving up so the test stays bounded.
    const SAMPLE_LIMIT: usize = 500;
    const MAX_SCAN: usize = 500_000;

    let mft = Mft::new(&volume);
    let mut iter = match mft.try_iter() {
        Ok(it) => it,
        Err(e) => {
            eprintln!("raw_mft_record_parity: skipping (Mft::try_iter failed: {e})");
            return;
        }
    };

    let mut samples: Vec<SampledMftEntry> = Vec::with_capacity(SAMPLE_LIMIT);
    let mut scanned = 0usize;

    while samples.len() < SAMPLE_LIMIT && scanned < MAX_SCAN {
        let Some(item) = iter.next() else {
            break;
        };
        scanned += 1;
        if let Ok(entry) = item {
            if entry.fid.is_standard() && !entry.file_name.is_empty() {
                samples.push(SampledMftEntry {
                    fid: entry.fid,
                    parent_fid: entry.parent_fid,
                    is_dir: entry.is_dir(),
                    file_name: entry.file_name,
                });
            }
        }
    }

    if samples.is_empty() {
        eprintln!(
            "raw_mft_record_parity: skipping \
             (no standard-ID entries found after scanning {scanned} records — \
             likely an extended-IDs-only environment)"
        );
        return;
    }

    let raw_mft = match RawMft::new(&volume) {
        Ok(m) => m,
        Err(UsnError::UnsupportedFilesystem(msg)) => {
            eprintln!("raw_mft_record_parity: skipping (unsupported filesystem: {msg})");
            return;
        }
        Err(e) => panic!("RawMft::new failed: {e}"),
    };

    let mut compared = 0usize;
    let mut agreements = 0usize;
    // Entries not found or already freed between the two reads.
    let mut transient_misses = 0usize;
    let mut logged_disagreements = 0usize;

    for entry in &samples {
        let record_number = match entry.fid.record_number() {
            Some(n) => n,
            None => continue,
        };

        let raw_entry = match raw_mft.get_record(record_number) {
            Ok(Some(r)) => r,
            // Record freed between the Mft scan and the RawMft lookup — fine.
            Ok(None) => {
                transient_misses += 1;
                continue;
            }
            Err(e) => panic!(
                "RawMft::get_record({record_number}) failed for fid {}: {e}",
                entry.fid
            ),
        };

        if !raw_entry.is_used {
            // Same: allocated then freed in the window between scans.
            transient_misses += 1;
            continue;
        }

        compared += 1;

        let same_fid    = raw_entry.file_reference == entry.fid;
        let same_parent = raw_entry.parent_reference == entry.parent_fid;
        let same_dir    = raw_entry.is_directory == entry.is_dir;
        // Case-insensitive: NTFS is case-insensitive and the two APIs may
        // normalise differently for short names vs long names.
        let same_name   = raw_entry
            .file_name
            .to_string_lossy()
            .eq_ignore_ascii_case(&entry.file_name.to_string_lossy());

        if same_fid && same_parent && same_dir && same_name {
            agreements += 1;
        } else {
            if logged_disagreements < 10 {
                logged_disagreements += 1;
                eprintln!(
                    "raw_mft_record_parity: disagreement record={record_number} \
                     fid_ok={same_fid} parent_ok={same_parent} dir_ok={same_dir} name_ok={same_name}\n  \
                     raw: name='{}' parent={} dir={}\n  \
                     mft: name='{}' parent={} dir={}",
                    raw_entry.file_name.to_string_lossy(),
                    raw_entry.parent_reference,
                    raw_entry.is_directory,
                    entry.file_name.to_string_lossy(),
                    entry.parent_fid,
                    entry.is_dir,
                );
            }
        }
    }

    if compared == 0 {
        eprintln!(
            "raw_mft_record_parity: skipping assertion \
             (all {scanned} scanned records were transient misses — \
             filesystem churn too high to compare)"
        );
        return;
    }

    // Allow up to 5 % disagreement to absorb live-filesystem churn
    // (files renamed, moved, or recreated between the two reads).
    let agreement_rate = agreements as f64 / compared as f64;
    assert!(
        agreement_rate >= 0.95,
        "expected >= 95% field agreement between RawMft and Mft API on standard-ID records; \
         got {agreements}/{compared} ({:.1}%), transient_misses={transient_misses}",
        agreement_rate * 100.0
    );

    eprintln!(
        "raw_mft_record_parity: {agreements}/{compared} ({:.1}%) agreement, \
         transient_misses={transient_misses}",
        agreement_rate * 100.0
    );
}
