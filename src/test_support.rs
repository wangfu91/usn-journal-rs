//! Test-only helpers shared across the crate's inline `#[cfg(test)]`
//! modules. Centralises the most repeated `injectorpp` mock blocks and
//! convenience constructors so that individual test modules can stay
//! focused on the behaviour they're verifying.
//!
//! This module is gated on `#[cfg(test)]` and is not part of the public
//! API. It deliberately uses macros for the `DeviceIoControl` mock
//! because `injectorpp::func!` / `injectorpp::fake!` need the exact
//! function signature at compile time and cannot be hidden behind a
//! regular function call.

#![allow(unused_imports, unused_macros, dead_code)]

use windows::Win32::Foundation::HANDLE;

use crate::volume::{Volume, VolumeSource};

/// Build a [`Volume`] backed by a stub handle suitable for use in tests
/// where the underlying Win32 calls are mocked with `injectorpp`.
///
/// The default drive letter is `'T'` to match the bulk of pre-existing
/// tests; pass a different letter via [`mock_volume_with`].
#[inline]
pub(crate) fn mock_volume() -> Volume {
    mock_volume_with('T')
}

/// Build a [`Volume`] for tests with a caller-chosen drive letter.
#[inline]
pub(crate) fn mock_volume_with(drive_letter: char) -> Volume {
    Volume::mock(
        HANDLE(std::ptr::null_mut()),
        VolumeSource::DriveLetter(drive_letter),
    )
}

/// Mock `DeviceIoControl` so that every call returns the provided
/// `windows::core::Result<()>`-shaped expression. The macro injects the
/// fake into the supplied `injectorpp::interface::injector::InjectorPP`
/// instance for the lifetime of that injector.
///
/// # Example
/// ```ignore
/// use injectorpp::interface::injector::*;
/// let mut injector = InjectorPP::new();
/// crate::test_support::mock_device_io_control!(
///     injector,
///     Err(windows::core::Error::from(
///         windows::Win32::Foundation::ERROR_INVALID_HANDLE,
///     ))
/// );
/// ```
macro_rules! mock_device_io_control {
    ($injector:expr, $result:expr) => {{
        $injector
            .when_called(injectorpp::func!(
                unsafe{} fn (windows::Win32::System::IO::DeviceIoControl)(
                    windows::Win32::Foundation::HANDLE,
                    u32,
                    Option<*const std::ffi::c_void>,
                    u32,
                    Option<*mut std::ffi::c_void>,
                    u32,
                    Option<*mut u32>,
                    Option<*mut windows::Win32::System::IO::OVERLAPPED>
                ) -> windows::core::Result<()>
            ))
            .will_execute(injectorpp::fake!(
                func_type: unsafe fn(
                    _handle: windows::Win32::Foundation::HANDLE,
                    _control_code: u32,
                    _input: Option<*const std::ffi::c_void>,
                    _input_size: u32,
                    _output: Option<*mut std::ffi::c_void>,
                    _output_size: u32,
                    _bytes_returned: Option<*mut u32>,
                    _overlapped: Option<*mut windows::Win32::System::IO::OVERLAPPED>
                ) -> windows::core::Result<()>,
                returns: $result
            ));
    }};
}

pub(crate) use mock_device_io_control;
