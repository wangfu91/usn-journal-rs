use std::{
    ffi::{OsString, c_void},
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use log::{debug, warn};
use windows::{
    Win32::{
        Foundation::{self, HANDLE},
        Storage::FileSystem::{
            self, CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_ID_DESCRIPTOR,
            FILE_SHARE_READ, FILE_SHARE_WRITE, GetVolumeNameForVolumeMountPointW, OPEN_EXISTING,
        },
    },
    core::HSTRING,
};

/// Obtain a handle to a volume for USN change journal operations using a drive letter.
///
/// # Arguments
/// * `drive_letter` - The drive letter of the volume (e.g., 'C').
///
/// # Returns
/// * `Ok(HANDLE)` - Handle to the volume.
/// * `Err(anyhow::Error)` - If the handle cannot be obtained.
pub fn get_volume_handle(drive_letter: char) -> anyhow::Result<HANDLE> {
    // https://learn.microsoft.com/en-us/windows/win32/fileio/obtaining-a-volume-handle-for-change-journal-operations
    // To obtain a handle to a volume for use with update sequence number (USN) change journal operations,
    // call the CreateFile function with the lpFileName parameter set to a string of the following form: \\.\X:
    // Note that X is the letter that identifies the drive on which the NTFS volume appears.
    let volume_root = format!(r"\\.\{}:", drive_letter);

    let volume_handle = unsafe {
        CreateFileW(
            &HSTRING::from(&volume_root),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
            None,
        )
        .with_context(|| {
            format!(
                "CreateFileW failed, volume_root={}, error={:?}",
                volume_root,
                Foundation::GetLastError()
            )
        })?
    };

    Ok(volume_handle)
}

/// Obtain a handle to a volume for USN change journal operations using a mount point path.
///
/// # Arguments
/// * `mount_point` - Path to the mount point (e.g., `C:\` or a mounted folder).
///
/// # Returns
/// * `Ok(HANDLE)` - Handle to the volume.
/// * `Err(anyhow::Error)` - If the handle cannot be obtained.
pub fn get_volume_handle_from_mount_point(mount_point: &Path) -> anyhow::Result<HANDLE> {
    // GetVolumeNameForVolumeMountPointW requires trailing backslash
    let mount_path = format!("{}\\", mount_point.to_string_lossy());

    let mut volume_name = [0u16; 50]; // Enough space for volume GUID path
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
    let name_data = volume_name
        .get(..end)
        .context("Failed to get volume name data")?;
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
        )
        .with_context(|| {
            format!(
                "CreateFileW failed, volume_path={}, error={:?}",
                volume_path,
                Foundation::GetLastError()
            )
        })?
    };

    Ok(volume_handle)
}

/// Resolve a file ID to its full path on the given volume.
///
/// # Arguments
/// * `volume_handle` - Handle to the NTFS volume.
/// * `drive_letter` - The drive letter (e.g., 'C').
/// * `file_id` - The file ID to resolve.
///
/// # Returns
/// * `Ok(PathBuf)` - The resolved full path.
/// * `Err(anyhow::Error)` - If the path cannot be resolved.
pub fn file_id_to_path(
    volume_handle: HANDLE,
    drive_letter: char,
    file_id: u64,
) -> anyhow::Result<PathBuf> {
    let file_id_desc = FILE_ID_DESCRIPTOR {
        Type: FileSystem::FileIdType,
        dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
        Anonymous: FileSystem::FILE_ID_DESCRIPTOR_0 {
            FileId: file_id.try_into()?,
        },
    };

    let file_handle = unsafe {
        FileSystem::OpenFileById(
            volume_handle,
            &file_id_desc,
            FileSystem::FILE_GENERIC_READ.0,
            FileSystem::FILE_SHARE_READ
                | FileSystem::FILE_SHARE_WRITE
                | FileSystem::FILE_SHARE_DELETE,
            None,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
        )
        .with_context(|| format!("OpenFileById failed, file_id={}", file_id))?
    };

    let init_len = size_of::<u32>() + (Foundation::MAX_PATH as usize) * size_of::<u16>();
    let mut info_buffer = vec![0u8; init_len];

    loop {
        if let Err(err) = unsafe {
            FileSystem::GetFileInformationByHandleEx(
                file_handle,
                FileSystem::FileNameInfo,
                &mut *info_buffer as *mut _ as *mut c_void,
                info_buffer.len() as u32,
            )
        } {
            if err.code() == Foundation::ERROR_MORE_DATA.into() {
                // Long paths, needs to extend buffer size to hold it.
                let name_info = unsafe {
                    std::ptr::read(info_buffer.as_ptr() as *const FileSystem::FILE_NAME_INFO)
                };

                let needed_len = name_info.FileNameLength + size_of::<u32>() as u32;
                // expand info_buffer capacity to needed_len to hold the long path
                info_buffer.resize(needed_len as usize, 0);
                // try again
                continue;
            }

            return Err(err.into());
        }

        break;
    }

    unsafe { Foundation::CloseHandle(file_handle) }?;

    let (_, body, _) = unsafe { info_buffer.align_to::<FileSystem::FILE_NAME_INFO>() };
    let info = body.first().context("Failed to read FILE_NAME_INFO")?;
    let name_len = info.FileNameLength as usize / size_of::<u16>();
    let name_u16 = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
    let sub_path = OsString::from_wide(name_u16);
    // Only convert to uppercase if it's lowercase
    let drive_char = if drive_letter.is_ascii_lowercase() {
        drive_letter.to_ascii_uppercase()
    } else {
        drive_letter
    };

    // Create the full path directly with a single allocation
    let mut full_path = PathBuf::new();
    full_path.push(format!("{}:\\", drive_char));
    full_path.push(sub_path);
    Ok(full_path)
}

/// Convert a Windows FILETIME (number of 100-nanosecond intervals since 1601-01-01 UTC)
/// to a `std::time::SystemTime`.
///
/// # Arguments
/// * `filetime` - The FILETIME value as i64.
///
/// # Returns
/// * `Ok(SystemTime)` - The corresponding system time.
/// * `Err(anyhow::Error)` - If the conversion fails.
pub fn filetime_to_systemtime(filetime: i64) -> anyhow::Result<SystemTime> {
    // Constant defining the number of 100-nanosecond intervals between the Windows epoch (1601-01-01)
    // and the Unix epoch (1970-01-01).
    // Corrected value: 116_444_736_000_000_000
    const EPOCH_DIFFERENCE_100NS: u64 = 116_444_736_000_000_000;

    // FILETIME is technically unsigned, representing intervals. Treat as u64.
    let filetime_u64 = filetime as u64;

    // Calculate the duration relative to the Unix epoch in 100ns units.
    let duration_since_unix_epoch_100ns = if filetime_u64 >= EPOCH_DIFFERENCE_100NS {
        // Time is at or after the Unix epoch
        filetime_u64 - EPOCH_DIFFERENCE_100NS
    } else {
        // Time is before the Unix epoch
        // Calculate the difference from the Unix epoch
        EPOCH_DIFFERENCE_100NS - filetime_u64
    };

    // Convert 100ns units to seconds and nanoseconds.
    let secs = duration_since_unix_epoch_100ns / 10_000_000;
    // Remainder is in 100ns units, convert to nanoseconds.
    let nanos = (duration_since_unix_epoch_100ns % 10_000_000) * 100;

    // Create a Duration object.
    let duration = Duration::new(secs, nanos as u32); // nanos fits in u32

    // Construct the SystemTime relative to the Unix epoch.
    let system_time = if filetime_u64 >= EPOCH_DIFFERENCE_100NS {
        UNIX_EPOCH + duration
    } else {
        UNIX_EPOCH - duration
    };

    Ok(system_time)
}

#[cfg(test)]
mod tests {
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::SystemTimeToFileTime,
    };

    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn filetime_to_systemtime_test() -> anyhow::Result<()> {
        // Test with the Unix Epoch (January 1, 1970 00:00:00 UTC)
        let unix_epoch_filetime: i64 = 116_444_736_000_000_000;
        let unix_epoch_systemtime = filetime_to_systemtime(unix_epoch_filetime)?;
        assert_eq!(unix_epoch_systemtime, UNIX_EPOCH);

        // Test with a date before Unix Epoch (Windows epoch: 1601-01-01 00:00:00 UTC)
        let windows_epoch_filetime: i64 = 0;
        let windows_epoch_systemtime = filetime_to_systemtime(windows_epoch_filetime)?;
        // Duration between 1601-01-01 and 1970-01-01 is 11644473600 seconds
        let expected = UNIX_EPOCH - Duration::from_secs(11644473600);
        assert_eq!(windows_epoch_systemtime, expected);

        // Test using SystemTimeToFileTime conversion for a specific date (2020-01-01)
        let st = SYSTEMTIME {
            wYear: 2020,
            wMonth: 1,
            wDay: 1,
            wDayOfWeek: 0,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        };
        let mut ft = FILETIME::default();
        unsafe {
            SystemTimeToFileTime(&st, &mut ft)
                .ok()
                .context("SystemTimeToFileTime failed for 2020")?;
        }
        let filetime_i64 = ((ft.dwHighDateTime as i64) << 32) | (ft.dwLowDateTime as i64);
        let converted_systemtime = filetime_to_systemtime(filetime_i64)?;
        // Manually construct expected SystemTime for 2020-01-01 00:00:00 UTC
        let expected = UNIX_EPOCH + Duration::from_secs(1577836800); // 2020-01-01T00:00:00Z
        assert_eq!(converted_systemtime, expected);

        // Test another date (2023-07-15 12:30:45)
        let st2 = SYSTEMTIME {
            wYear: 2023,
            wMonth: 7,
            wDay: 15,
            wDayOfWeek: 0,
            wHour: 12,
            wMinute: 30,
            wSecond: 45,
            wMilliseconds: 0,
        };
        let mut ft2 = FILETIME::default();
        unsafe {
            SystemTimeToFileTime(&st2, &mut ft2)
                .ok()
                .context("SystemTimeToFileTime failed for 2023")?;
        }
        let filetime_i64_2 = ((ft2.dwHighDateTime as i64) << 32) | (ft2.dwLowDateTime as i64);
        let converted_systemtime2 = filetime_to_systemtime(filetime_i64_2)?;
        // 2023-07-15T12:30:45Z = 1689424245 seconds since UNIX_EPOCH
        let expected2 = UNIX_EPOCH + Duration::from_secs(1689424245);
        assert_eq!(converted_systemtime2, expected2);

        Ok(())
    }
}
