//! Volume handle management for NTFS/ReFS

use crate::{errors::UsnError, privilege};
use log::{debug, warn};
use std::path::Path;
use windows::{
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, HANDLE},
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_SHARE_READ,
            FILE_SHARE_WRITE, GetVolumeNameForVolumeMountPointW, OPEN_EXISTING,
        },
    },
    core::HSTRING,
};

#[derive(Debug, Clone)]
/// Represents an NTFS/ReFS volume handle and its associated drive letter or mount point.
pub struct Volume {
    pub(crate) handle: HANDLE,
    pub drive_letter: Option<char>,
    pub mount_point: Option<String>,
}

impl Volume {
    /// Creates a new `Volume` instance with the given drive letter.
    pub fn from_drive_letter(drive_letter: char) -> Result<Self, UsnError> {
        let handle = get_volume_handle_from_drive_letter(drive_letter)?;
        Ok(Volume {
            handle,
            drive_letter: Some(drive_letter),
            mount_point: None,
        })
    }

    /// Creates a new `Volume` instance with the given mount point.
    pub fn from_mount_point(mount_point: &Path) -> Result<Self, UsnError> {
        let handle = get_volume_handle_from_mount_point(mount_point)?;
        Ok(Volume {
            handle,
            drive_letter: None,
            mount_point: Some(mount_point.to_string_lossy().to_string()),
        })
    }
}

/// Opens a handle to an NTFS/ReFS volume using a drive letter.
fn get_volume_handle_from_drive_letter(drive_letter: char) -> Result<HANDLE, UsnError> {
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

/// Opens a handle to an NTFS/ReFS volume using a mount point path.
fn get_volume_handle_from_mount_point(mount_point: &Path) -> Result<HANDLE, UsnError> {
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
