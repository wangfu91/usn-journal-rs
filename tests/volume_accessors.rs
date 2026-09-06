#![cfg(windows)]

//! Integration tests for the `Volume` convenience accessors
//! (`journal()`, `mft()`, `path_resolver()`) and a couple of
//! MFT enumeration edge cases.
//!
//! Each accessor must be equivalent to the explicit constructor it wraps.
//! Tests run against the `USN_TEST_DRIVE` volume (default `C`) and skip
//! gracefully when the process is not elevated or the drive is unavailable.

use usn_journal_rs::{
    Usn,
    errors::UsnError,
    journal::UsnJournal,
    mft::{Mft, MftIterOptions},
    path::PathResolver,
    volume::Volume,
};

/// Pick the NTFS test volume, honoring `USN_TEST_DRIVE` (default `C`).
fn pick_drive() -> char {
    std::env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

/// Open the test volume, or return `None` with a skip message.
fn open_test_volume(test_name: &str) -> Option<Volume> {
    let drive = pick_drive();
    match Volume::from_drive_letter(drive) {
        Ok(volume) => Some(volume),
        Err(UsnError::NotElevated) => {
            eprintln!("{test_name}: skipping (requires admin privileges)");
            None
        }
        Err(error) => {
            eprintln!("{test_name}: skipping on {drive}: {error}");
            None
        }
    }
}

#[test]
fn journal_accessor_matches_constructor() {
    let Some(volume) = open_test_volume("journal_accessor_matches_constructor") else {
        return;
    };

    let via_accessor = volume
        .journal()
        .query_or_create()
        .expect("accessor journal query_or_create should succeed");
    let via_constructor = UsnJournal::new(&volume)
        .query_or_create()
        .expect("constructor journal query_or_create should succeed");

    assert_eq!(
        via_accessor.journal_id, via_constructor.journal_id,
        "volume.journal() must wrap the same journal as UsnJournal::new"
    );
}

#[test]
fn mft_accessor_matches_constructor_first_entries() {
    let Some(volume) = open_test_volume("mft_accessor_matches_constructor_first_entries") else {
        return;
    };

    // The first enumerated MFT records are NTFS metafiles ($MFT, $MFTMirr,
    // $LogFile, ...) whose file references are stable, so a bounded prefix
    // read back-to-back must be identical between the two entry points.
    const PREFIX: usize = 32;

    let collect_prefix = |iter: usn_journal_rs::mft::MftIter| -> Vec<_> {
        iter.filter_map(Result::ok)
            .take(PREFIX)
            .map(|entry| entry.fid)
            .collect::<Vec<_>>()
    };

    let via_accessor = collect_prefix(volume.mft().try_iter().expect("accessor mft try_iter"));
    let via_constructor = collect_prefix(
        Mft::new(&volume)
            .try_iter()
            .expect("constructor mft try_iter"),
    );

    assert!(
        !via_accessor.is_empty(),
        "MFT enumeration yielded no entries"
    );
    assert_eq!(
        via_accessor, via_constructor,
        "volume.mft() must enumerate the same records as Mft::new"
    );
}

#[test]
fn path_resolver_accessor_matches_constructor() {
    let Some(volume) = open_test_volume("path_resolver_accessor_matches_constructor") else {
        return;
    };

    let accessor_resolver = volume.path_resolver();
    let constructor_resolver = PathResolver::new(&volume);

    let mut compared = 0usize;
    for entry in Mft::new(&volume)
        .try_iter()
        .expect("mft try_iter")
        .filter_map(Result::ok)
        .filter(|e| !e.file_name.is_empty())
        .take(200)
    {
        let via_accessor = accessor_resolver.resolve_path(&entry);
        let via_constructor = constructor_resolver.resolve_path(&entry);
        assert_eq!(
            via_accessor, via_constructor,
            "accessor and constructor resolvers must agree for fid {}",
            entry.fid
        );
        if via_accessor.is_some() {
            compared += 1;
        }
    }

    assert!(
        compared > 0,
        "expected at least one resolvable entry in the first 200 MFT records"
    );
}

#[test]
fn mft_high_usn_zero_yields_no_entries() {
    let Some(volume) = open_test_volume("mft_high_usn_zero_yields_no_entries") else {
        return;
    };

    // A HighUsn of 0 means "only records whose USN is <= 0". Real files always
    // have a positive USN, so this must yield an empty enumeration. This also
    // guards the fix that no longer seeds the enumeration cursor from low_usn:
    // enumeration still starts at record 0 and filters purely by USN.
    let options = MftIterOptions::builder()
        .low_usn(Usn::new(0))
        .high_usn(Usn::new(0))
        .build();

    let mut yielded = 0usize;
    for result in volume
        .mft()
        .try_iter_with_options(options)
        .expect("try_iter")
    {
        match result {
            Ok(_) => yielded += 1,
            Err(e) => panic!("unexpected error during bounded MFT enumeration: {e}"),
        }
        if yielded > 0 {
            break;
        }
    }

    assert_eq!(
        yielded, 0,
        "HighUsn=0 should filter out every real record, but {yielded} were returned"
    );
}

#[test]
fn mft_full_range_yields_many_entries() {
    let Some(volume) = open_test_volume("mft_full_range_yields_many_entries") else {
        return;
    };

    // Sanity check on the default (full) USN range: enumeration starting at
    // record 0 must return a healthy number of records.
    let count = volume
        .mft()
        .try_iter()
        .expect("try_iter")
        .filter_map(Result::ok)
        .take(1_000)
        .count();

    assert!(
        count >= 100,
        "expected the full-range MFT scan to yield many records, got {count}"
    );
}
