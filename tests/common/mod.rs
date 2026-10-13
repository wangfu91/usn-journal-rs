//! Environment checks for tests that require a live NTFS volume.

use usn_journal_rs::{UsnResult, privilege, volume::Volume};
use windows::{
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_INVALID_DRIVE, ERROR_NOT_READY, ERROR_PATH_NOT_FOUND,
        },
        Storage::FileSystem::GetVolumeInformationW,
    },
    core::HSTRING,
};

pub fn open_test_volume(test_name: &str) -> UsnResult<Option<Volume>> {
    if !privilege::is_elevated()? {
        eprintln!("{test_name}: skipping (requires admin privileges)");
        return Ok(None);
    }
    let drive = std::env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C');
    let root = format!("{drive}:\\");
    let mut filesystem = [0u16; 64];
    // SAFETY: the root string and writable filesystem buffer live for this call.
    if let Err(error) = unsafe {
        GetVolumeInformationW(
            &HSTRING::from(&root),
            None,
            None,
            None,
            None,
            Some(&mut filesystem),
        )
    } {
        if [
            ERROR_FILE_NOT_FOUND,
            ERROR_PATH_NOT_FOUND,
            ERROR_INVALID_DRIVE,
            ERROR_NOT_READY,
        ]
        .iter()
        .any(|code| error.code() == (*code).into())
        {
            eprintln!("{test_name}: skipping (volume {root} unavailable: {error})");
            return Ok(None);
        }
        return Err(error.into());
    }
    let end = filesystem
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(filesystem.len());
    if !String::from_utf16_lossy(&filesystem[..end]).eq_ignore_ascii_case("NTFS") {
        eprintln!("{test_name}: skipping ({root} is not NTFS)");
        return Ok(None);
    }
    // After prerequisites pass, failures must fail the test rather than skip it.
    Volume::from_drive_letter(drive).map(Some)
}
