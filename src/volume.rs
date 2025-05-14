//! Utility functions for NTFS volume.

use crate::{errors::UsnError, privilege};
use log::{debug, warn};
use std::path::Path;
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, HANDLE},
        Storage::FileSystem::{
            CreateFileW, GetVolumeNameForVolumeMountPointW, FILE_FLAGS_AND_ATTRIBUTES,
            FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    },
};

/// Opens a handle to an NTFS volume for USN change journal operations using a drive letter.
///
/// # Arguments
/// * `drive_letter` - The drive letter of the NTFS volume (e.g., 'C').
///
/// # Returns
/// * `Ok(HANDLE)` - Handle to the NTFS volume.
/// * `Err(anyhow::Error)` - If the handle cannot be opened.
pub(crate) fn get_volume_handle(drive_letter: char) -> Result<HANDLE, UsnError> {
    if !privilege::is_elevated()? {
        return Err(UsnError::PermissionError);
    }

    // https://learn.microsoft.com/en-us/windows/win32/fileio/obtaining-a-volume-handle-for-change-journal-operations
    // To obtain a handle to a volume for use with update sequence number (USN) change journal operations,
    // call the CreateFile function with the lpFileName parameter set to a string of the following form: \\.\X:
    // Note that X is the letter that identifies the drive on which the NTFS volume appears.
    let volume_root = format!(r"\\.\{}:", drive_letter);

    match unsafe {
        CreateFileW(
            &HSTRING::from(&volume_root),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
            None,
        )
    } {
        Ok(handle) => Ok(handle),
        Err(err) if err == ERROR_ACCESS_DENIED.into() => Err(UsnError::PermissionError),
        Err(err) => Err(UsnError::WinApiError(err)),
    }
}

/// Opens a handle to an NTFS volume for USN change journal operations using a mount point path.
///
/// # Arguments
/// * `mount_point` - Path to the mount point (e.g., `C:\` or a mounted folder).
///
/// # Returns
/// * `Ok(HANDLE)` - Handle to the NTFS volume.
/// * `Err(anyhow::Error)` - If the handle cannot be opened.
pub(crate) fn get_volume_handle_from_mount_point(mount_point: &Path) -> Result<HANDLE, UsnError> {
    if !privilege::is_elevated()? {
        return Err(UsnError::PermissionError);
    }

    // GetVolumeNameForVolumeMountPointW requires trailing backslash
    let mount_path = format!("{}\\", mount_point.to_string_lossy());

    let mut volume_name = [0u16; 64]; // Enough space for volume GUID path
    if let Err(err) =
        unsafe { GetVolumeNameForVolumeMountPointW(&HSTRING::from(&mount_path), &mut volume_name) }
    {
        warn!(
            "GetVolumeNameForVolumeMountPointW failed, mount_point={}, error={:?}",
            mount_path, err
        );
        return Err(err.into());
    }

    // Convert the null-terminated wide string to a Rust string
    let end = volume_name
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(volume_name.len());
    let name_data = volume_name.get(..end).ok_or(UsnError::OtherError(
        "Failed to get volume name data".to_string(),
    ))?;
    let volume_guid = String::from_utf16_lossy(name_data);

    debug!("Volume GUID: {}", volume_guid);

    // IMPORTANT: Remove the trailing backslash for CreateFileW
    let volume_path = volume_guid.trim_end_matches('\\').to_string();
    debug!("Using volume path: {}", volume_path);

    let volume_handle = unsafe {
        CreateFileW(
            &HSTRING::from(&volume_path),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
            None,
        )?
    };

    Ok(volume_handle)
}
