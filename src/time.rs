use crate::errors::UsnError;
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
/// * `Result<SystemTime, UsnError>` - The corresponding system time or an error for invalid input.
///
/// # Errors
/// * Returns an error if the filetime value is negative, as FILETIME values should be non-negative.
pub(crate) fn filetime_to_systemtime(filetime: i64) -> Result<SystemTime, UsnError> {
    // FILETIME is technically unsigned, representing 100-nanosecond intervals.
    // Negative values are invalid and should be rejected.
    if filetime < 0 {
        return Err(UsnError::OtherError(format!(
            "FILETIME cannot be negative: {filetime}"
        )));
    }

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
    Ok(system_time_utc.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::UsnError;
    use std::time::{Duration, UNIX_EPOCH};
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME as WinSystemTime},
        System::Time::SystemTimeToFileTime,
    };

    // Basic functionality tests
    mod basic_conversion_tests {
        use super::*;

        #[test]
        fn filetime_to_systemtime_test() -> windows::core::Result<()> {
            // Test with the Unix Epoch (January 1, 1970 00:00:00 UTC)
            let unix_epoch_filetime: i64 = 116_444_736_000_000_000;
            let unix_epoch_systemtime = filetime_to_systemtime(unix_epoch_filetime).unwrap();
            assert_eq!(unix_epoch_systemtime, UNIX_EPOCH);

            // Test with a date before Unix Epoch (Windows epoch: 1601-01-01 00:00:00 UTC)
            let windows_epoch_filetime: i64 = 0;
            let windows_epoch_systemtime = filetime_to_systemtime(windows_epoch_filetime).unwrap();
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
            let converted_systemtime = filetime_to_systemtime(filetime_i64).unwrap();

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
            let converted_systemtime2 = filetime_to_systemtime(filetime_i64_2).unwrap();

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

    // Edge case and boundary tests
    mod edge_case_tests {
        use super::*;

        #[test]
        fn test_large_filetime_values() {
            // Test with a reasonable large FILETIME value (not MAX to avoid overflow)
            let large_filetime = 132_000_000_000_000_000; // Year ~2020
            let result = filetime_to_systemtime(large_filetime).unwrap();

            // Should not panic and should produce a valid SystemTime
            assert!(result > UNIX_EPOCH);
        }

        #[test]
        fn test_negative_filetime_values() {
            // Test with negative FILETIME (should return error)
            let negative_filetime = -10_000_000; // 1 second before Windows epoch
            let result = filetime_to_systemtime(negative_filetime);

            assert!(result.is_err());
            if let Err(UsnError::OtherError(msg)) = result {
                assert!(msg.contains("FILETIME cannot be negative"));
            } else {
                panic!("Expected OtherError with negative FILETIME message");
            }
        }

        #[test]
        fn test_extremely_large_filetime_values() {
            // Test with very large FILETIME values that might cause overflow
            let max_safe_value = i64::MAX - 1;
            let result = filetime_to_systemtime(max_safe_value);

            // This should either succeed or fail gracefully, not panic
            match result {
                Ok(_) => {
                    // If it succeeds, the result should be valid
                }
                Err(_) => {
                    // If it fails, that's also acceptable for extreme values
                }
            }
        }

        #[test]
        fn test_overflow_edge_cases() {
            // Test values that might cause arithmetic overflow
            let near_overflow = i64::MAX / 10_000_000 - 1;
            let overflow_seconds = near_overflow * 10_000_000;

            let result = filetime_to_systemtime(overflow_seconds);
            // Should handle gracefully without panicking
            assert!(result.is_ok() || result.is_err());
        }

        #[test]
        fn test_nanosecond_precision() {
            // Test that nanosecond precision is handled correctly
            let base_filetime = 116_444_736_000_000_000; // Unix epoch

            // Add exactly 1 second (10,000,000 * 100-nanosecond intervals)
            let one_second_later = base_filetime + 10_000_000;
            let result = filetime_to_systemtime(one_second_later).unwrap();
            let expected = UNIX_EPOCH + Duration::from_secs(1);
            assert_eq!(result, expected);

            // Add exactly 1 millisecond (10,000 * 100-nanosecond intervals)
            let one_ms_later = base_filetime + 10_000;
            let result_ms = filetime_to_systemtime(one_ms_later).unwrap();
            let expected_ms = UNIX_EPOCH + Duration::from_millis(1);
            assert_eq!(result_ms, expected_ms);
        }

        #[test]
        fn test_filetime_as_unsigned() {
            // Test conversion with a reasonable large value
            let large_value = 130_000_000_000_000_000_i64; // Well after Unix epoch
            let result = filetime_to_systemtime(large_value).unwrap();

            // Should not panic and should produce a SystemTime after Unix epoch
            assert!(result > UNIX_EPOCH);
        }
    }

    // Performance and consistency tests
    mod consistency_tests {
        use super::*;

        #[test]
        fn test_conversion_consistency() {
            // Test that converting maintains reasonable accuracy
            let test_values = vec![
                0,                       // Windows epoch
                116_444_736_000_000_000, // Unix epoch
                132_103_584_000_000_000, // 2020-01-01
            ];

            for filetime in test_values {
                let system_time = filetime_to_systemtime(filetime).unwrap();

                // Convert back to approximate FILETIME for comparison
                let duration_since_windows_epoch = system_time
                    .duration_since(WINDOWS_EPOCH_UTC.into())
                    .unwrap_or_else(|_| Duration::new(0, 0));

                // Convert to 100-nanosecond intervals (FILETIME units)
                let reconstructed_intervals = duration_since_windows_epoch.as_nanos() / 100;
                let reconstructed_filetime = reconstructed_intervals as u64;

                // Allow for reasonable precision differences (within 1 second)
                let diff = (filetime as u64).abs_diff(reconstructed_filetime);

                // Allow for precision differences within 1 second (10M intervals)
                assert!(
                    diff < 10_000_000,
                    "Conversion inconsistency: {filetime} vs {reconstructed_filetime} (diff: {diff})"
                );
            }
        }
    }

    // Integration tests with actual Windows API
    mod integration_tests {
        use super::*;

        #[test]
        fn test_current_time_conversion() -> windows::core::Result<()> {
            // Get a known FILETIME and test the conversion
            // Using a fixed time for predictable testing
            let st = WinSystemTime {
                wYear: 2024,
                wMonth: 1,
                wDay: 1,
                wDayOfWeek: 0,
                wHour: 12,
                wMinute: 0,
                wSecond: 0,
                wMilliseconds: 0,
            };

            // Convert to FILETIME
            let mut ft = FILETIME::default();
            unsafe {
                SystemTimeToFileTime(&st, &mut ft)?;
            }

            // Convert to i64 and then to SystemTime using our function
            let filetime_i64 = ((ft.dwHighDateTime as i64) << 32) | (ft.dwLowDateTime as i64);
            let converted = filetime_to_systemtime(filetime_i64).unwrap();

            // Should match the expected time
            let expected_dt = DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
                Utc,
            );
            let expected: SystemTime = expected_dt.into();
            assert_eq!(converted, expected);

            Ok(())
        }
    }
}
