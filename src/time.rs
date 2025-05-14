use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Converts a Windows FILETIME (100-nanosecond intervals since 1601-01-01 UTC)
/// to a `std::time::SystemTime`.
///
/// # Arguments
/// * `filetime` - FILETIME value as i64.
///
/// # Returns
/// * `SystemTime` - The corresponding system time.
pub fn filetime_to_systemtime(filetime: i64) -> SystemTime {
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

    if filetime_u64 >= EPOCH_DIFFERENCE_100NS {
        UNIX_EPOCH + duration
    } else {
        UNIX_EPOCH - duration
    }
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
    fn filetime_to_systemtime_test() -> windows::core::Result<()> {
        // Test with the Unix Epoch (January 1, 1970 00:00:00 UTC)
        let unix_epoch_filetime: i64 = 116_444_736_000_000_000;
        let unix_epoch_systemtime = filetime_to_systemtime(unix_epoch_filetime);
        assert_eq!(unix_epoch_systemtime, UNIX_EPOCH);

        // Test with a date before Unix Epoch (Windows epoch: 1601-01-01 00:00:00 UTC)
        let windows_epoch_filetime: i64 = 0;
        let windows_epoch_systemtime = filetime_to_systemtime(windows_epoch_filetime);
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
        unsafe { SystemTimeToFileTime(&st, &mut ft)? };
        let filetime_i64 = ((ft.dwHighDateTime as i64) << 32) | (ft.dwLowDateTime as i64);
        let converted_systemtime = filetime_to_systemtime(filetime_i64);
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
        unsafe { SystemTimeToFileTime(&st2, &mut ft2)? };
        let filetime_i64_2 = ((ft2.dwHighDateTime as i64) << 32) | (ft2.dwLowDateTime as i64);
        let converted_systemtime2 = filetime_to_systemtime(filetime_i64_2);
        // 2023-07-15T12:30:45Z = 1689424245 seconds since UNIX_EPOCH
        let expected2 = UNIX_EPOCH + Duration::from_secs(1689424245);
        assert_eq!(converted_systemtime2, expected2);

        Ok(())
    }
}
