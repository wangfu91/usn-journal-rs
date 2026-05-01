//! Volume handle management for NTFS/ReFS

use crate::{errors::UsnError, privilege};
use log::{debug, warn};
use std::path::{Path, PathBuf};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE},
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_SHARE_READ,
            FILE_SHARE_WRITE, GetVolumeNameForVolumeMountPointW, OPEN_EXISTING,
        },
    },
    core::HSTRING,
};

/// The source used to open a [`Volume`].
#[derive(Debug)]
pub enum VolumeSource {
    /// Volume was opened via a drive letter (e.g. `'C'`).
    DriveLetter(char),
    /// Volume was opened via a mount point path.
    MountPoint(PathBuf),
}

#[derive(Debug)]
/// Represents an NTFS/ReFS volume handle and its associated drive letter or mount point.
pub struct Volume {
    pub(crate) handle: HANDLE,
    source: VolumeSource,
}

impl Volume {
    /// Creates a new `Volume` instance with the given drive letter.
    pub fn from_drive_letter(drive_letter: char) -> Result<Self, UsnError> {
        let handle = get_volume_handle_from_drive_letter(drive_letter)?;
        Ok(Volume {
            handle,
            source: VolumeSource::DriveLetter(drive_letter),
        })
    }

    /// Creates a new `Volume` instance with the given mount point.
    pub fn from_mount_point<P: AsRef<Path>>(mount_point: P) -> Result<Self, UsnError> {
        let path = mount_point.as_ref();
        let handle = get_volume_handle_from_mount_point(path)?;
        Ok(Volume {
            handle,
            source: VolumeSource::MountPoint(path.to_path_buf()),
        })
    }

    /// Returns the source used to open this volume.
    #[must_use]
    #[inline]
    pub fn source(&self) -> &VolumeSource {
        &self.source
    }

    /// Returns the drive letter if this volume was opened via a drive letter.
    #[must_use]
    #[inline]
    pub fn drive_letter(&self) -> Option<char> {
        match &self.source {
            VolumeSource::DriveLetter(c) => Some(*c),
            _ => None,
        }
    }

    /// Returns the mount point path if this volume was opened via a mount point.
    #[must_use]
    #[inline]
    pub fn mount_point(&self) -> Option<&Path> {
        match &self.source {
            VolumeSource::MountPoint(p) => Some(p),
            _ => None,
        }
    }

    /// Returns the raw volume handle.
    #[inline]
    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }

    /// Creates a mock `Volume` for testing (invalid handle, no real device).
    #[cfg(test)]
    pub(crate) fn mock(handle: HANDLE, source: VolumeSource) -> Self {
        Volume { handle, source }
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        if self.handle.is_invalid() {
            return;
        }

        // SAFETY: `self.handle` was returned by a successful `CreateFileW`
        // call (see `from_drive_letter` / `from_mount_point`) and has not
        // been closed elsewhere. `Drop` runs at most once per `Volume`,
        // so this is the unique close. `CloseHandle` is safe to call on
        // a valid handle that the caller owns.
        if let Err(err) = unsafe { CloseHandle(self.handle) } {
            warn!("Failed to close volume handle: {err}");
        }
    }
}

/// Opens a handle to an NTFS/ReFS volume using a drive letter.
fn get_volume_handle_from_drive_letter(drive_letter: char) -> Result<HANDLE, UsnError> {
    if !privilege::is_elevated()? {
        return Err(UsnError::NotElevated);
    }

    // https://learn.microsoft.com/en-us/windows/win32/fileio/obtaining-a-volume-handle-for-change-journal-operations
    // To obtain a handle to a volume for use with update sequence number (USN) change journal operations,
    // call the CreateFile function with the lpFileName parameter set to a string of the following form: \\.\X:
    // Note that X is the letter that identifies the drive on which the NTFS volume appears.
    let volume_root = format!(r"\\.\{drive_letter}:");

    // SAFETY: All pointer/reference parameters point to valid local
    // values constructed just above. `CreateFileW` accepts a wide-string
    // file name (provided through `HSTRING`) and may return either a
    // valid handle or a Win32 error; both outcomes are handled below.
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
        Err(err) if err == ERROR_ACCESS_DENIED.into() => Err(UsnError::NotElevated),
        Err(err) => Err(UsnError::WinApi(err)),
    }
}

/// Opens a handle to an NTFS/ReFS volume using a mount point path.
fn get_volume_handle_from_mount_point(mount_point: &Path) -> Result<HANDLE, UsnError> {
    if !privilege::is_elevated()? {
        return Err(UsnError::NotElevated);
    }

    // GetVolumeNameForVolumeMountPointW requires trailing backslash
    let mount_path = format!("{}\\", mount_point.to_string_lossy());

    let mut volume_name = [0u16; 50]; // Enough space for volume GUID path
    // SAFETY: `mount_path` lives until the end of the call; `volume_name`
    // is a stack buffer of u16 we are writing into. The Win32 contract
    // for `GetVolumeNameForVolumeMountPointW` only requires the buffer
    // be large enough to hold the volume GUID path (50 wide chars is
    // ample for the standard `\\?\Volume{GUID}\` form).
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
    let name_data = volume_name
        .get(..end)
        .ok_or_else(|| UsnError::InvalidMountPointError("Failed to get volume name data".into()))?;
    let volume_guid = String::from_utf16_lossy(name_data);

    debug!("Volume GUID: {volume_guid}");

    // IMPORTANT: Remove the trailing backslash for CreateFileW
    let volume_path = volume_guid.trim_end_matches('\\').to_string();
    debug!("Using volume path: {volume_path}");

    // SAFETY: `volume_path` is a valid null-terminated wide string for
    // the duration of the call (held by the `HSTRING`). All other pointer
    // parameters are either `None` or owned defaults. The returned handle
    // (or error) is propagated to the caller, who becomes the owner.
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
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;

    use crate::{errors::UsnError, volume::Volume};

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
                        volume.drive_letter(),
                        Some(drive_letter),
                        "Drive letter should match"
                    );
                    assert!(volume.mount_point().is_none(), "Mount point should be None");
                    Ok(())
                }
                Err(UsnError::NotElevated) => {
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
                Err(UsnError::NotElevated) => {
                    eprintln!("Got permission error - test requires admin privileges");
                }
                Err(UsnError::WinApi(err)) if err.code() == ERROR_FILE_NOT_FOUND.into() => {
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
            let result = Volume::from_mount_point(std::path::Path::new(mount_point));
            eprintln!("Result: {result:?}");
            assert!(
                result.is_err(),
                "Should return an error for invalid mount point"
            );
        }
    }
}
