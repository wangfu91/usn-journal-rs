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
    let volume_root = format!(r"\\.\{drive_letter}:");

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

    let mut volume_name = [0u16; 50]; // Enough space for volume GUID path
    if let Err(err) =
        unsafe { GetVolumeNameForVolumeMountPointW(&HSTRING::from(&mount_path), &mut volume_name) }
    {
        warn!("GetVolumeNameForVolumeMountPointW failed, mount_point={mount_path}, error={err:?}");
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

    debug!("Volume GUID: {volume_guid}");

    // IMPORTANT: Remove the trailing backslash for CreateFileW
    let volume_path = volume_guid.trim_end_matches('\\').to_string();
    debug!("Using volume path: {volume_path}");

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

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HANDLE};

    use crate::{errors::UsnError, volume::Volume};

    // Unit tests for Volume struct behavior
    mod unit_tests {
        use super::*;

        #[test]
        fn test_volume_debug_formatting() {
            let volume = Volume {
                handle: HANDLE(std::ptr::null_mut()),
                drive_letter: Some('C'),
                mount_point: None,
            };
            let debug_str = format!("{volume:?}");
            assert!(debug_str.contains("handle"));
            assert!(debug_str.contains("drive_letter"));
            assert!(debug_str.contains("mount_point"));
        }

        #[test]
        fn test_volume_clone() {
            let original = Volume {
                handle: HANDLE(std::ptr::null_mut()),
                drive_letter: Some('D'),
                mount_point: Some("D:\\mount".to_string()),
            };

            let cloned = original.clone();
            assert_eq!(original.handle, cloned.handle);
            assert_eq!(original.drive_letter, cloned.drive_letter);
            assert_eq!(original.mount_point, cloned.mount_point);
        }

        #[test]
        fn test_volume_struct_variants() {
            // Test drive letter only
            let vol1 = Volume {
                handle: HANDLE(std::ptr::null_mut()),
                drive_letter: Some('C'),
                mount_point: None,
            };
            assert!(vol1.drive_letter.is_some());
            assert!(vol1.mount_point.is_none());

            // Test mount point only
            let vol2 = Volume {
                handle: HANDLE(std::ptr::null_mut()),
                drive_letter: None,
                mount_point: Some("C:\\mount".to_string()),
            };
            assert!(vol2.drive_letter.is_none());
            assert!(vol2.mount_point.is_some());

            // Test both (edge case)
            let vol3 = Volume {
                handle: HANDLE(std::ptr::null_mut()),
                drive_letter: Some('D'),
                mount_point: Some("D:\\mount".to_string()),
            };
            assert!(vol3.drive_letter.is_some());
            assert!(vol3.mount_point.is_some());
        }
    }

    // Integration tests that require actual filesystem access
    mod integration_tests {
        use super::*;

        #[test]
        fn test_get_volume_handle_from_valid_drive_letter() -> Result<(), UsnError> {
            let drive_letter = 'C';
            match Volume::from_drive_letter(drive_letter) {
                Ok(volume) => {
                    assert!(!volume.handle.is_invalid(), "Volume handle should be valid");
                    assert_eq!(
                        volume.drive_letter,
                        Some(drive_letter),
                        "Drive letter should match"
                    );
                    assert!(volume.mount_point.is_none(), "Mount point should be None");
                    Ok(())
                }
                Err(UsnError::PermissionError) => {
                    eprintln!("Skipping test - requires admin privileges");
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }

        #[test]
        fn test_get_volume_handle_from_invalid_drive_letter() {
            let drive_letter = 'W'; // Assuming W is not a valid drive letter
            let result = Volume::from_drive_letter(drive_letter);

            // The test should always return an error - either permission denied or file not found
            assert!(
                result.is_err(),
                "Should return an error for invalid drive letter"
            );

            // Log the specific error for debugging purposes
            match result {
                Err(UsnError::PermissionError) => {
                    eprintln!("Got permission error - test requires admin privileges");
                }
                Err(UsnError::WinApiError(err)) if err.code() == ERROR_FILE_NOT_FOUND.into() => {
                    eprintln!("Got expected file not found error");
                }
                Err(other_err) => {
                    eprintln!("Got other error (acceptable): {other_err:?}");
                }
                Ok(_) => {
                    panic!("Unexpected success - drive W should not exist");
                }
            }
        }

        #[test]
        fn test_get_volume_handle_from_invalid_mount_point() {
            let mount_point = r"C:\invalid\mount\point";
            let result = Volume::from_mount_point(mount_point.as_ref());
            eprintln!("Result: {result:?}");
            assert!(
                result.is_err(),
                "Should return an error for invalid mount point"
            );
        }
    }
}
