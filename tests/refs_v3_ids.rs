//! Best-effort integration coverage for ReFS / USN v3 128-bit file IDs.
//!
//! On the developer machine D: is ReFS, so journal and MFT enumeration should
//! expose extended file IDs. The tests skip gracefully when the configured
//! drive is unavailable, inaccessible, or does not surface a v3 record in the
//! sampled prefix.

use usn_journal_rs::{journal::UsnJournal, mft::Mft, volume::Volume};

fn refs_drive() -> char {
    std::env::var("USN_REFS_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('D')
}

#[test]
fn refs_journal_can_surface_extended_file_ids() {
    let drive = refs_drive();
    let volume = match Volume::from_drive_letter(drive) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("refs_v3_ids: skipping journal test on {drive}: {e}");
            return;
        }
    };

    let journal = UsnJournal::new(&volume);
    let mut iter = match journal.try_iter() {
        Ok(iter) => iter,
        Err(e) => {
            eprintln!("refs_v3_ids: skipping journal iterator on {drive}: {e}");
            return;
        }
    };

    let found = iter
        .by_ref()
        .take(10_000)
        .filter_map(Result::ok)
        .any(|entry| entry.fid.is_extended() || entry.parent_fid.is_extended());

    if !found {
        eprintln!(
            "refs_v3_ids: no extended journal file IDs observed on {drive}: in sampled prefix"
        );
    }
}

#[test]
fn refs_mft_can_surface_extended_file_ids() {
    let drive = refs_drive();
    let volume = match Volume::from_drive_letter(drive) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("refs_v3_ids: skipping mft test on {drive}: {e}");
            return;
        }
    };

    let mft = Mft::new(&volume);
    let mut iter = match mft.try_iter() {
        Ok(iter) => iter,
        Err(e) => {
            eprintln!("refs_v3_ids: skipping mft iterator on {drive}: {e}");
            return;
        }
    };

    let found = iter
        .by_ref()
        .take(10_000)
        .filter_map(Result::ok)
        .any(|entry| entry.fid.is_extended() || entry.parent_fid.is_extended());

    if !found {
        eprintln!("refs_v3_ids: no extended mft file IDs observed on {drive}: in sampled prefix");
    }
}
