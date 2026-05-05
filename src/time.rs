//! Conversion helpers and the public `Filetime` wrapper.

use crate::errors::UsnError;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::FILETIME;

/// Number of 100-nanosecond intervals between the Windows FILETIME epoch
/// (1601-01-01 UTC) and the Unix epoch (1970-01-01 UTC).
pub(crate) const WINDOWS_TO_UNIX_OFFSET_100NS: u64 = 116_444_736_000_000_000u64;

/// A Windows `FILETIME` value: a count of 100-nanosecond intervals since
/// 1601-01-01 UTC.
///
/// This is the raw timestamp representation used by the NTFS USN journal
/// and MFT records. It is exposed in this crate's public API in place of
/// any specific date/time type so that callers can pick whichever
/// downstream conversion suits their use case.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Filetime(u64);

impl Filetime {
    /// Construct a `Filetime` from its raw 100-ns interval count.
    #[must_use]
    #[inline]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the raw 100-ns interval count since the Windows epoch.
    #[must_use]
    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Convert to `SystemTime`.
    ///
    /// Returns `None` only when the resulting `SystemTime` cannot be
    /// represented on the current platform.
    #[must_use]
    pub fn to_system_time(self) -> Option<SystemTime> {
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

    /// Convert from `SystemTime`.
    ///
    /// Returns `None` when the input is before the Windows FILETIME epoch or
    /// when the 100-nanosecond interval count would overflow `u64`.
    #[must_use]
    pub fn from_system_time(value: SystemTime) -> Option<Self> {
        system_time_to_filetime_raw(value).map(Self)
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
}

impl From<FILETIME> for Filetime {
    #[inline]
    fn from(value: FILETIME) -> Self {
        Self(((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64)
    }
}

impl From<Filetime> for FILETIME {
    #[inline]
    fn from(value: Filetime) -> Self {
        Self {
            dwLowDateTime: value.raw() as u32,
            dwHighDateTime: (value.raw() >> 32) as u32,
        }
    }
}

impl TryFrom<SystemTime> for Filetime {
    type Error = UsnError;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        Self::from_system_time(value).ok_or(UsnError::InvalidTimestamp(
            "SystemTime is outside the Windows FILETIME range",
        ))
    }
}

impl TryFrom<Filetime> for SystemTime {
    type Error = UsnError;

    fn try_from(value: Filetime) -> Result<Self, Self::Error> {
        value.to_system_time().ok_or(UsnError::InvalidTimestamp(
            "FILETIME is outside the SystemTime range",
        ))
    }
}

/// Convert a `SystemTime` into its raw Windows `FILETIME` representation.
fn system_time_to_filetime_raw(value: SystemTime) -> Option<u64> {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let intervals = duration.as_nanos() / 100;
            (WINDOWS_TO_UNIX_OFFSET_100NS as u128)
                .checked_add(intervals)
                .and_then(|raw| u64::try_from(raw).ok())
        }
        Err(err) => {
            let intervals = err.duration().as_nanos() / 100;
            if intervals > WINDOWS_TO_UNIX_OFFSET_100NS as u128 {
                None
            } else {
                Some(WINDOWS_TO_UNIX_OFFSET_100NS - intervals as u64)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME as WinSystemTime},
        System::Time::SystemTimeToFileTime,
    };

    // Basic functionality tests
    mod basic_conversion_tests {
        use super::*;

        #[test]
        fn filetime_unix_and_windows_epoch() {
            // Test with the Unix Epoch (January 1, 1970 00:00:00 UTC)
            let unix_epoch_filetime: u64 = 116_444_736_000_000_000;
            let unix_epoch_systemtime =
                Filetime::new(unix_epoch_filetime).to_system_time().unwrap();
            assert_eq!(unix_epoch_systemtime, UNIX_EPOCH);

            // Test with a date before Unix Epoch (Windows epoch: 1601-01-01 00:00:00 UTC)
            let windows_epoch_systemtime = Filetime::new(0).to_system_time().unwrap();
            let secs_between_epochs = 116_444_736_000_000_000 / 10_000_000;
            let expected = UNIX_EPOCH - Duration::from_secs(secs_between_epochs);
            assert_eq!(windows_epoch_systemtime, expected);
        }

        #[test]
        fn filetime_round_trip_via_win32() -> windows::core::Result<()> {
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
            let filetime = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
            let converted = Filetime::new(filetime).to_system_time().unwrap();

            // Reconstruct: SystemTime -> FILETIME via nanos and compare.
            let dur = converted.duration_since(UNIX_EPOCH).unwrap();
            let intervals = dur.as_nanos() / 100;
            let unix_epoch_filetime: u128 = 116_444_736_000_000_000;
            assert_eq!(filetime as u128, intervals + unix_epoch_filetime);
            Ok(())
        }
    }

    // Edge case and boundary tests
    mod edge_case_tests {
        use super::*;

        #[test]
        fn test_large_filetime_values() {
            // Test with a reasonable large FILETIME value (not MAX to avoid overflow)
            let large_filetime = 132_000_000_000_000_000u64; // Year ~2020
            let result = Filetime::new(large_filetime).to_system_time().unwrap();

            // Should not panic and should produce a valid SystemTime
            assert!(result > UNIX_EPOCH);
        }

        #[test]
        fn test_nanosecond_precision() {
            // Test that nanosecond precision is handled correctly
            let base_filetime = 116_444_736_000_000_000u64; // Unix epoch

            // Add exactly 1 second (10,000,000 * 100-nanosecond intervals)
            let one_second_later = base_filetime + 10_000_000;
            let result = Filetime::new(one_second_later).to_system_time().unwrap();
            let expected = UNIX_EPOCH + Duration::from_secs(1);
            assert_eq!(result, expected);

            // Add exactly 1 millisecond (10,000 * 100-nanosecond intervals)
            let one_ms_later = base_filetime + 10_000;
            let result_ms = Filetime::new(one_ms_later).to_system_time().unwrap();
            let expected_ms = UNIX_EPOCH + Duration::from_millis(1);
            assert_eq!(result_ms, expected_ms);
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

            let windows_epoch_systemtime = Filetime::new(0).to_system_time().unwrap();

            for filetime in test_values {
                let system_time = Filetime::new(filetime).to_system_time().unwrap();

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

        #[test]
        fn unix_epoch_boundary() {
            let f = Filetime::new(WINDOWS_TO_UNIX_OFFSET_100NS);
            assert_eq!(f.to_system_time(), Some(UNIX_EPOCH));
            assert_eq!(f.to_unix_seconds(), 0);
            assert_eq!(f.to_unix_nanos(), 0);
        }

        #[test]
        fn underflow_below_unix_epoch() {
            // 1 second before Unix epoch in FILETIME units.
            let f = Filetime::new(WINDOWS_TO_UNIX_OFFSET_100NS - 10_000_000);
            // Should still be representable (1969-12-31 23:59:59) on platforms
            // where SystemTime supports pre-Unix-epoch times.
            assert_eq!(f.to_unix_seconds(), -1);
        }

        #[test]
        fn zero_is_windows_epoch() {
            let f = Filetime::new(0);
            // Windows epoch: 1601-01-01. Should be representable as
            // SystemTime on Windows.
            let st = f.to_system_time().expect("windows epoch");
            assert!(st < UNIX_EPOCH);
        }

        #[test]
        fn new_raw_and_win32_conversions_round_trip() {
            let raw = 0x0123_4567_89ab_cdef;
            let filetime = Filetime::new(raw);
            assert_eq!(filetime.raw(), raw);
            assert_eq!(Filetime::new(raw), filetime);

            let win32 = FILETIME {
                dwLowDateTime: raw as u32,
                dwHighDateTime: (raw >> 32) as u32,
            };
            let filetime_from_win32 = Filetime::from(win32);
            assert_eq!(filetime_from_win32.raw(), raw);

            let round_trip_win32: FILETIME = filetime_from_win32.into();
            assert_eq!(round_trip_win32.dwLowDateTime, win32.dwLowDateTime);
            assert_eq!(round_trip_win32.dwHighDateTime, win32.dwHighDateTime);
        }

        #[test]
        fn from_system_time_round_trips_unix_epoch() {
            let filetime = Filetime::from_system_time(UNIX_EPOCH).expect("unix epoch");
            assert_eq!(filetime.raw(), WINDOWS_TO_UNIX_OFFSET_100NS);
            let system_time: SystemTime = filetime.try_into().expect("system time");
            assert_eq!(system_time, UNIX_EPOCH);
        }

        #[test]
        fn try_from_system_time_preserves_subsecond_ticks() {
            let st = UNIX_EPOCH + Duration::new(1, 123_456_700);
            let filetime = Filetime::try_from(st).expect("system time");
            assert_eq!(
                filetime.raw(),
                WINDOWS_TO_UNIX_OFFSET_100NS + 10_000_000 + 1_234_567
            );
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

            let filetime = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
            let converted = Filetime::new(filetime).to_system_time().unwrap();

            let dur = converted.duration_since(UNIX_EPOCH).unwrap();
            let intervals = dur.as_nanos() / 100;
            let unix_epoch_filetime: u128 = 116_444_736_000_000_000;
            assert_eq!(filetime as u128, intervals + unix_epoch_filetime);

            Ok(())
        }
    }
}
