#![cfg(windows)]
#![allow(deprecated)] // Exercise names retained for callers migrating from 0.4.1.

//! Compile and behavior checks for the restored public compatibility names.

use usn_journal_rs::{USN_REASON_MASK_ALL, UsnError, UsnReason, journal, mft};

#[test]
fn legacy_options_names_accept_the_validated_options_types() {
    let journal_options: journal::EnumOptions = journal::JournalIterOptions::builder()
        .reason_mask(UsnReason::from_bits_retain(USN_REASON_MASK_ALL))
        .build()
        .expect("valid journal options");
    assert_eq!(journal_options.reason_mask().bits(), u32::MAX);
    assert!(!journal::EnumOptions::default().wait_for_more());

    let mft_options: mft::EnumOptions = mft::MftIterOptions::builder()
        .build()
        .expect("valid MFT options");
    assert_eq!(
        mft_options.high_usn(),
        mft::EnumOptions::default().high_usn()
    );
}

#[test]
fn other_error_remains_an_importable_and_matchable_variant() {
    use usn_journal_rs::UsnError::OtherError;
    let error = OtherError("application context".into());
    assert_eq!(error.to_string(), "Other error: application context");
    assert!(!error.is_parse_error());
    assert!(!error.is_permission_denied());
    assert!(matches!(error, UsnError::OtherError(message) if message == "application context"));
}
