use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, NaiveDateTime, Utc};
use std::time::SystemTime;

// Define the Windows epoch as a const.
// NaiveDate/Time construction can panic if given invalid values, but 1601-01-01 00:00:00 is valid.
const WINDOWS_EPOCH_NAIVE: NaiveDateTime = match NaiveDate::from_ymd_opt(1601, 1, 1) {
    Some(date) => match date.and_hms_opt(0, 0, 0) {
        Some(datetime) => datetime,
        // These panics should ideally not be hit for hardcoded valid dates/times.
        None => panic!("Invalid time component for Windows epoch constant"),
    },
    None => panic!("Invalid date component for Windows epoch constant"),
};
const WINDOWS_EPOCH_UTC: DateTime<Utc> =
    DateTime::<Utc>::from_naive_utc_and_offset(WINDOWS_EPOCH_NAIVE, Utc);

/// Converts a Windows FILETIME (100-nanosecond intervals since 1601-01-01 UTC)
/// to a `std::time::SystemTime`.
///
/// # Arguments
/// * `filetime` - FILETIME value as i64.
///
/// # Returns
/// * `SystemTime` - The corresponding system time.
pub(crate) fn filetime_to_systemtime(filetime: i64) -> SystemTime {
    // FILETIME is technically unsigned, representing 100-nanosecond intervals.
    let filetime_u64 = filetime as u64;

    // Convert 100-nanosecond intervals to seconds and remaining nanoseconds.
    let secs_since_windows_epoch = filetime_u64 / 10_000_000;
    let nanos_remainder = (filetime_u64 % 10_000_000) * 100;

    // Create a chrono::Duration from these parts.
    let duration_since_windows_epoch = ChronoDuration::seconds(secs_since_windows_epoch as i64)
        + ChronoDuration::nanoseconds(nanos_remainder as i64);

    // Add this duration to the Windows epoch.
    let system_time_utc = WINDOWS_EPOCH_UTC + duration_since_windows_epoch;

    // Convert chrono::DateTime<Utc> to std::time::SystemTime.
    system_time_utc.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME as WinSystemTime},
        System::Time::SystemTimeToFileTime,
    };

    #[test]
    fn filetime_to_systemtime_test() -> windows::core::Result<()> {
        // Test with the Unix Epoch (January 1, 1970 00:00:00 UTC)
        let unix_epoch_filetime: i64 = 116_444_736_000_000_000;
        let unix_epoch_systemtime = filetime_to_systemtime(unix_epoch_filetime);
        assert_eq!(unix_epoch_systemtime, UNIX_EPOCH);

        // Test with a date before Unix Epoch (Windows epoch: 1601-01-01 00:00:00 UTC)
        let windows_epoch_filetime: i64 = 0;
        let windows_epoch_systemtime = filetime_to_systemtime(windows_epoch_filetime);
        // Duration between 1601-01-01 and 1970-01-01
        // This is equivalent to EPOCH_DIFFERENCE_100NS / 10_000_000
        let secs_between_epochs = 116_444_736_000_000_000 / 10_000_000;
        let expected = UNIX_EPOCH - Duration::from_secs(secs_between_epochs);
        assert_eq!(windows_epoch_systemtime, expected);

        // Test using SystemTimeToFileTime conversion for a specific date (2020-01-01)
        let st = WinSystemTime {
            // Use aliased WinSystemTime
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

        let expected_dt_2020 = DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDate::from_ymd_opt(2020, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            Utc,
        );
        let expected: SystemTime = expected_dt_2020.into();
        assert_eq!(converted_systemtime, expected);

        // Test another date (2023-07-15 12:30:45)
        let st2 = WinSystemTime {
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

        let expected_dt_2023 = DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDate::from_ymd_opt(2023, 7, 15)
                .unwrap()
                .and_hms_opt(12, 30, 45)
                .unwrap(),
            Utc,
        );
        let expected2: SystemTime = expected_dt_2023.into();
        assert_eq!(converted_systemtime2, expected2);

        Ok(())
    }
}
