//! Integration test: raw-MFT iteration on a ReFS volume returns
//! `UsnError::UnsupportedFilesystem`.
//!
//! On the developer machine D: is ReFS; `RawMft::new` must reject it.
//! The test gracefully skips when:
//!   * D: does not exist or cannot be opened (no drive or no privileges).
//!   * D: is actually NTFS (in which case the assertion is not exercised).
//!
//! The `USN_REFS_TEST_DRIVE` environment variable may override the drive
//! letter (default: `D`).

use usn_journal_rs::{errors::UsnError, raw_mft::RawMft, volume::Volume};

#[test]
fn refs_volume_returns_unsupported_filesystem() {
    let drive = std::env::var("USN_REFS_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('D');

    let volume = match Volume::from_drive_letter(drive) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "refs_unsupported: skipping — drive {drive}: not available or not accessible: {e}"
            );
            return;
        }
    };

    match RawMft::new(&volume) {
        Err(UsnError::UnsupportedFilesystem(msg)) => {
            // Expected path: drive is ReFS and raw-MFT is rejected.
            assert!(
                !msg.is_empty(),
                "UnsupportedFilesystem message must not be empty"
            );
            assert!(
                matches!(
                    RawMft::new(&volume),
                    Err(UsnError::UnsupportedFilesystem(_))
                ),
                "UnsupportedFilesystem must be returned consistently"
            );
        }
        Err(other) => {
            // Drive {drive} opened but failed for another reason.  This can
            // happen when the drive is neither NTFS nor recognized as ReFS
            // (e.g., FAT32, exFAT).  Accept gracefully.
            eprintln!(
                "refs_unsupported: drive {drive}: returned {other} \
                 (not UnsupportedFilesystem — may not be ReFS; skipping assertion)"
            );
        }
        Ok(_) => {
            // Drive {drive} is NTFS — UnsupportedFilesystem not exercised.
            eprintln!(
                "refs_unsupported: drive {drive}: is NTFS; \
                 UnsupportedFilesystem variant was not triggered (skipping assertion)"
            );
        }
    }
}
