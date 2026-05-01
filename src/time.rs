use crate::errors::UsnError;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Number of 100-nanosecond intervals between the Windows FILETIME epoch
/// (1601-01-01 UTC) and the Unix epoch (1970-01-01 UTC).
pub(crate) const WINDOWS_TO_UNIX_OFFSET_100NS: u64 = 116_444_736_000_000_000u64;

/// A Windows `FILETIME` value: a count of 100-nanosecond intervals since
/// 1601-01-01 UTC.
///
/// This is the raw timestamp representation used by the NTFS USN journal
/// and MFT records. It is exposed in this crate's public API in place of
/// any specific date/time type so that callers can pick whichever
/// downstream conversion (e.g. `SystemTime`, `chrono::DateTime<Utc>`,
/// `time::OffsetDateTime`) suits their use case.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Filetime(pub u64);

impl Filetime {
    /// Build a `Filetime` from a Win32 `FILETIME` struct.
    #[must_use]
    #[inline]
    pub fn from_filetime_struct(ft: windows::Win32::Foundation::FILETIME) -> Self {
        let value = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
        Filetime(value)
    }

    /// Build a `Filetime` from the signed 64-bit representation used by
    /// USN journal records. Negative inputs are clamped to zero.
    #[must_use]
    #[inline]
    pub fn from_raw_i64(i: i64) -> Self {
        if i < 0 {
            Filetime(0)
        } else {
            Filetime(i as u64)
        }
    }

    /// Build a `Filetime` from a raw `u64`.
    #[must_use]
    #[inline]
    pub const fn from_u64(u: u64) -> Self {
        Filetime(u)
    }

    /// Raw 100-ns interval count since the Windows epoch.
    #[must_use]
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Try to convert to `SystemTime`. Returns `None` only when the
    /// resulting `SystemTime` cannot be represented on the current
    /// platform (Windows-epoch underflow on systems that lack pre-Unix
    /// `SystemTime` support, etc.).
    #[must_use]
    pub fn try_to_system_time(self) -> Option<SystemTime> {
        if self.0 >= WINDOWS_TO_UNIX_OFFSET_100NS {
            let intervals = self.0 - WINDOWS_TO_UNIX_OFFSET_100NS;
            let secs = intervals / 10_000_000;
            let nanos = ((intervals % 10_000_000) * 100) as u32;
            UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
        } else {
            let intervals = WINDOWS_TO_UNIX_OFFSET_100NS - self.0;
            let secs = intervals / 10_000_000;
            let nanos = ((intervals % 10_000_000) * 100) as u32;
            UNIX_EPOCH.checked_sub(Duration::new(secs, nanos))
        }
    }

    /// Convert to `SystemTime`, falling back to `UNIX_EPOCH` on failure.
    #[must_use]
    pub fn to_system_time_or_epoch(self) -> SystemTime {
        self.try_to_system_time().unwrap_or(UNIX_EPOCH)
    }

    /// Number of seconds since the Unix epoch (may be negative).
    #[must_use]
    #[inline]
    pub fn to_unix_seconds(self) -> i64 {
        let intervals = self.0 as i128 - WINDOWS_TO_UNIX_OFFSET_100NS as i128;
        (intervals / 10_000_000) as i64
    }

    /// Number of nanoseconds since the Unix epoch (may be negative).
    #[must_use]
    #[inline]
    pub fn to_unix_nanos(self) -> i128 {
        let intervals = self.0 as i128 - WINDOWS_TO_UNIX_OFFSET_100NS as i128;
        intervals * 100
    }

    /// Convert this FILETIME to a `chrono::DateTime<Utc>`. Returns
    /// `None` if the value is out of `chrono`'s representable range.
    ///
    /// This method is only available when the `chrono` crate feature is
    /// enabled.
    #[cfg(feature = "chrono")]
    #[must_use]
    pub fn to_chrono_utc(self) -> Option<chrono::DateTime<chrono::Utc>> {
        let secs = self.to_unix_seconds();
        let nanos = ((self.0 % 10_000_000) * 100) as u32;
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
    }
}

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
#[allow(dead_code)]
pub(crate) fn filetime_to_systemtime(filetime: i64) -> Result<SystemTime, UsnError> {
    // FILETIME is technically unsigned, representing 100-nanosecond intervals.
    // Negative values are invalid and should be rejected.
    if filetime < 0 {
        return Err(UsnError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("FILETIME cannot be negative: {filetime}"),
        )));
    }

    Filetime::from_u64(filetime as u64)
        .try_to_system_time()
        .ok_or_else(|| {
            UsnError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("FILETIME out of representable range: {filetime}"),
            ))
        })
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
        fn filetime_to_systemtime_unix_and_windows_epoch() {
            // Test with the Unix Epoch (January 1, 1970 00:00:00 UTC)
            let unix_epoch_filetime: i64 = 116_444_736_000_000_000;
            let unix_epoch_systemtime = filetime_to_systemtime(unix_epoch_filetime).unwrap();
            assert_eq!(unix_epoch_systemtime, UNIX_EPOCH);

            // Test with a date before Unix Epoch (Windows epoch: 1601-01-01 00:00:00 UTC)
            let windows_epoch_filetime: i64 = 0;
            let windows_epoch_systemtime = filetime_to_systemtime(windows_epoch_filetime).unwrap();
            let secs_between_epochs = 116_444_736_000_000_000 / 10_000_000;
            let expected = UNIX_EPOCH - Duration::from_secs(secs_between_epochs);
            assert_eq!(windows_epoch_systemtime, expected);
        }

        #[test]
        fn filetime_to_systemtime_round_trip_via_win32() -> windows::core::Result<()> {
            // Use SystemTimeToFileTime to get a Win32-blessed FILETIME, then
            // convert to SystemTime and back, asserting nanosecond accuracy.
            let st = WinSystemTime {
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
            let converted = filetime_to_systemtime(filetime_i64).unwrap();

            // Reconstruct: SystemTime -> FILETIME via nanos and compare.
            let dur = converted.duration_since(UNIX_EPOCH).unwrap();
            let intervals = dur.as_nanos() / 100;
            let unix_epoch_filetime: u128 = 116_444_736_000_000_000;
            assert_eq!(filetime_i64 as u128, intervals + unix_epoch_filetime);
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
            if let Err(UsnError::Io(e)) = result {
                assert!(e.to_string().contains("FILETIME cannot be negative"));
            } else {
                panic!("Expected Io error with negative FILETIME message");
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
                0u64,                    // Windows epoch
                116_444_736_000_000_000, // Unix epoch
                132_103_584_000_000_000, // 2020-01-01
            ];

            let windows_epoch_systemtime = filetime_to_systemtime(0).unwrap();

            for filetime in test_values {
                let system_time = filetime_to_systemtime(filetime as i64).unwrap();

                // Convert back to approximate FILETIME for comparison
                let duration_since_windows_epoch = system_time
                    .duration_since(windows_epoch_systemtime)
                    .unwrap_or_else(|_| Duration::new(0, 0));

                // Convert to 100-nanosecond intervals (FILETIME units)
                let reconstructed_intervals = duration_since_windows_epoch.as_nanos() / 100;
                let reconstructed_filetime = reconstructed_intervals as u64;

                // Allow for reasonable precision differences (within 1 second)
                let diff = filetime.abs_diff(reconstructed_filetime);

                // Allow for precision differences within 1 second (10M intervals)
                assert!(
                    diff < 10_000_000,
                    "Conversion inconsistency: {filetime} vs {reconstructed_filetime} (diff: {diff})"
                );
            }
        }
    }

    mod filetime_newtype_tests {
        use super::*;
        use windows::Win32::Foundation::FILETIME;

        #[test]
        fn round_trip_from_filetime_struct() {
            let value: u64 = 132_000_000_000_000_000;
            let ft = FILETIME {
                dwLowDateTime: (value & 0xFFFF_FFFF) as u32,
                dwHighDateTime: (value >> 32) as u32,
            };
            let f = Filetime::from_filetime_struct(ft);
            assert_eq!(f.as_u64(), value);
        }

        #[test]
        fn unix_epoch_boundary() {
            let f = Filetime::from_u64(WINDOWS_TO_UNIX_OFFSET_100NS);
            assert_eq!(f.try_to_system_time(), Some(UNIX_EPOCH));
            assert_eq!(f.to_unix_seconds(), 0);
            assert_eq!(f.to_unix_nanos(), 0);
        }

        #[test]
        fn underflow_below_unix_epoch() {
            // 1 second before Unix epoch in FILETIME units.
            let f = Filetime::from_u64(WINDOWS_TO_UNIX_OFFSET_100NS - 10_000_000);
            // Should still be representable (1969-12-31 23:59:59) on platforms
            // where SystemTime supports pre-Unix-epoch times.
            assert_eq!(f.to_unix_seconds(), -1);
        }

        #[test]
        fn zero_is_windows_epoch() {
            let f = Filetime::from_u64(0);
            // Windows epoch: 1601-01-01. Should be representable as
            // SystemTime on Windows.
            let st = f.try_to_system_time().expect("windows epoch");
            assert!(st < UNIX_EPOCH);
        }

        #[test]
        fn from_raw_i64_clamps_negative() {
            assert_eq!(Filetime::from_raw_i64(-5).as_u64(), 0);
            assert_eq!(Filetime::from_raw_i64(42).as_u64(), 42);
        }
    }

    // Integration tests with actual Windows API
    mod integration_tests {
        use super::*;

        #[test]
        fn test_current_time_conversion() -> windows::core::Result<()> {
            // Get a known FILETIME, convert via our function, and round-trip
            // back to FILETIME units to confirm no precision loss.
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

            let mut ft = FILETIME::default();
            unsafe {
                SystemTimeToFileTime(&st, &mut ft)?;
            }

            let filetime_i64 = ((ft.dwHighDateTime as i64) << 32) | (ft.dwLowDateTime as i64);
            let converted = filetime_to_systemtime(filetime_i64).unwrap();

            let dur = converted.duration_since(UNIX_EPOCH).unwrap();
            let intervals = dur.as_nanos() / 100;
            let unix_epoch_filetime: u128 = 116_444_736_000_000_000;
            assert_eq!(filetime_i64 as u128, intervals + unix_epoch_filetime);

            Ok(())
        }
    }
}
