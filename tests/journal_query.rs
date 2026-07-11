#![cfg(windows)]

//! Integration tests for the split `UsnJournal::query` / `query_or_create` API.
//!
//! These exercise the real USN journal on the `USN_TEST_DRIVE` volume
//! (default `C`). They skip gracefully when the process is not elevated or
//! the drive is unavailable.

use usn_journal_rs::{errors::UsnError, volume::Volume};

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
fn query_or_create_returns_self_consistent_state() {
    let Some(volume) = open_test_volume("query_or_create_returns_self_consistent_state") else {
        return;
    };
    let journal = volume.journal();

    let data = journal
        .query_or_create()
        .expect("query_or_create should succeed on an NTFS volume");

    assert_ne!(data.journal_id, 0, "an active journal must have a non-zero id");
    assert!(
        data.first_usn <= data.next_usn,
        "first_usn ({}) must not exceed next_usn ({})",
        data.first_usn,
        data.next_usn
    );
    assert!(
        data.lowest_valid_usn <= data.next_usn,
        "lowest_valid_usn ({}) must not exceed next_usn ({})",
        data.lowest_valid_usn,
        data.next_usn
    );
    assert!(
        data.maximum_size > 0,
        "an active journal should report a non-zero maximum size"
    );
}

#[test]
fn query_succeeds_after_query_or_create() {
    let Some(volume) = open_test_volume("query_succeeds_after_query_or_create") else {
        return;
    };
    let journal = volume.journal();

    // Ensure the journal exists first.
    let created = journal
        .query_or_create()
        .expect("query_or_create should succeed on an NTFS volume");

    // A plain query must now succeed (no JournalNotActive) and agree on the id.
    let queried = journal
        .query()
        .expect("query should succeed once the journal is active");

    assert_eq!(
        created.journal_id, queried.journal_id,
        "query and query_or_create must report the same journal id"
    );
}

#[test]
fn query_or_create_is_idempotent() {
    let Some(volume) = open_test_volume("query_or_create_is_idempotent") else {
        return;
    };
    let journal = volume.journal();

    let first = journal
        .query_or_create()
        .expect("first query_or_create should succeed");
    let second = journal
        .query_or_create()
        .expect("second query_or_create should succeed");

    assert_eq!(
        first.journal_id, second.journal_id,
        "repeated query_or_create calls must not rotate the journal id"
    );
}
