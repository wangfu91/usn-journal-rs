#![cfg(windows)]

//! ReFS / USN v3 coverage using a file created on a verified ReFS volume.
//! Set USN_REFS_TEST_DRIVE to select the volume (default D).
//! Only environment checks skip; enumeration and fixture failures fail the test.
//! ReFS FSCTL enumeration support is checked against a direct Win32 call.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use usn_journal_rs::{Usn, UsnError, journal::JournalIterOptions, volume::Volume};
use windows::{
    Win32::{
        Foundation::{ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED},
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_SHARE_READ,
            FILE_SHARE_WRITE, GetVolumeInformationW, OPEN_EXISTING,
        },
        System::{
            IO::DeviceIoControl,
            Ioctl::{FSCTL_ENUM_USN_DATA, MFT_ENUM_DATA_V1},
        },
    },
    core::{HSTRING, Owned},
};

struct Fixture {
    volume: Volume,
    path: PathBuf,
    name: OsString,
    start_usn: Usn,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn fixture() -> Result<Option<Fixture>, Box<dyn std::error::Error>> {
    let drive = std::env::var("USN_REFS_TEST_DRIVE")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('D');
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
        eprintln!("skipping ReFS test: {root} unavailable: {error}");
        return Ok(None);
    }
    let end = filesystem
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(filesystem.len());
    if !String::from_utf16_lossy(&filesystem[..end]).eq_ignore_ascii_case("ReFS") {
        eprintln!("skipping ReFS test: {root} is not ReFS");
        return Ok(None);
    }
    let volume = match Volume::from_drive_letter(drive) {
        Ok(volume) => volume,
        Err(UsnError::PermissionError) => {
            eprintln!("skipping ReFS test: requires Administrator privileges");
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let start_usn = volume.journal().query_or_create()?.next_usn;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let name = OsString::from(format!("usn-rs-v3-{}-{nonce}.tmp", std::process::id()));
    let path = PathBuf::from(root).join(&name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let fixture = Fixture {
        volume,
        path,
        name,
        start_usn,
    };
    file.write_all(b"USN v3 regression fixture")?;
    file.sync_all()?;
    drop(file);
    Ok(Some(fixture))
}

#[test]
fn refs_journal_can_surface_extended_file_ids() {
    let Some(fixture) = fixture().expect("prepare ReFS fixture") else {
        return;
    };
    let options = JournalIterOptions::builder()
        .start_usn(fixture.start_usn)
        .wait_for_more(false)
        .build()
        .expect("valid iterator options");
    let entry = fixture
        .volume
        .journal()
        .try_iter_with_options(options)
        .expect("create ReFS journal iterator")
        .map(|result| result.expect("decode ReFS journal record"))
        .find(|entry| entry.file_name == fixture.name)
        .expect("journal must contain the newly created fixture");
    assert!(entry.fid.is_extended(), "fixture FID must be 128-bit");
    assert!(entry.parent_fid.is_extended(), "parent FID must be 128-bit");
}

#[test]
fn refs_mft_matches_native_enumeration_support() {
    let Some(fixture) = fixture().expect("prepare ReFS fixture") else {
        return;
    };
    let drive = fixture.volume.drive_letter().expect("drive-letter fixture");
    // SAFETY: the path lives for this call; Owned closes the returned handle.
    let handle = unsafe {
        Owned::new(
            CreateFileW(
                &HSTRING::from(format!(r"\\.\{drive}:")),
                FILE_GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES::default(),
                None,
            )
            .expect("open independent ReFS probe handle"),
        )
    };
    let input = MFT_ENUM_DATA_V1 {
        StartFileReferenceNumber: 0,
        LowUsn: 0,
        HighUsn: i64::MAX,
        MinMajorVersion: 2,
        MaxMajorVersion: 3,
    };
    let mut buffer = vec![0u8; usn_journal_rs::journal::DEFAULT_BUFFER_BYTES];
    let mut bytes_returned = 0;
    // SAFETY: both buffers and the returned-byte pointer are valid for the call.
    let native = unsafe {
        DeviceIoControl(
            *handle,
            FSCTL_ENUM_USN_DATA,
            Some((&input as *const MFT_ENUM_DATA_V1).cast()),
            std::mem::size_of::<MFT_ENUM_DATA_V1>() as u32,
            Some(buffer.as_mut_ptr().cast()),
            buffer.len() as u32,
            Some(&mut bytes_returned),
            None,
        )
    };
    if let Err(error) = native {
        assert!(
            error.code() == ERROR_INVALID_FUNCTION.into()
                || error.code() == ERROR_NOT_SUPPORTED.into(),
            "unexpected native ReFS enumeration failure: {error}"
        );
        let library_error = fixture
            .volume
            .mft()
            .iter()
            .next()
            .expect("unsupported enumeration must yield an error item")
            .expect_err("library must preserve native enumeration failure");
        assert!(
            matches!(library_error, UsnError::WinApiError(ref underlying) if underlying.code() == error.code()),
            "native ReFS error must survive unchanged: {library_error}"
        );
        eprintln!("Verified native ReFS enumeration limitation and OS error propagation: {error}");
        return;
    }
    let entry = fixture
        .volume
        .mft()
        .iter()
        .map(|result| result.expect("decode ReFS MFT record"))
        .find(|entry| entry.file_name == fixture.name)
        .expect("MFT enumeration must contain the newly created fixture");
    assert!(entry.fid.is_extended(), "fixture FID must be 128-bit");
    assert!(entry.parent_fid.is_extended(), "parent FID must be 128-bit");
}
