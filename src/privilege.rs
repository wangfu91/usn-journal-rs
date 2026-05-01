use std::mem::size_of;

use windows::Win32::{
    Foundation::HANDLE,
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};
use windows::core::Owned;

pub(crate) fn is_elevated() -> windows::core::Result<bool> {
    let mut handle: HANDLE = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle with the
    // necessary access; `&mut handle` is a valid out-pointer. The Win32
    // call writes a real token handle on success which we wrap in
    // `Owned` immediately so it is closed on scope exit.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle)? };
    // SAFETY: `handle` was initialised by the successful `OpenProcessToken`
    // call above and ownership has not been transferred elsewhere.
    let handle = unsafe { Owned::new(handle) };

    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned_length = 0;

    // SAFETY: `*handle` is a live token handle owned by the local
    // `Owned` wrapper; `&mut elevation` and `&mut returned_length` are
    // valid out-pointers, and the buffer length matches the size of
    // `TOKEN_ELEVATION` exactly.
    unsafe {
        GetTokenInformation(
            *handle,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        )?
    };

    Ok(elevation.TokenIsElevated != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use injectorpp::interface::injector::*;

    // Unit tests for privilege checking functionality
    mod privilege_tests {
        use super::*;

        #[test]
        fn test_is_elevated_returns_bool() {
            // Test that the function returns a Result<bool, _>
            match is_elevated() {
                Ok(_elevated) => {
                    // Function succeeded and returned a boolean value
                }
                Err(_) => {
                    // Function may fail on some systems, which is acceptable
                }
            }
        }
    }

    // Mocked tests for error scenarios
    mod mocked_privilege_tests {
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE};

        use super::*;

        #[test]
        fn test_open_process_token_failure() {
            let mut injector = InjectorPP::new();

            // Mock OpenProcessToken to return an error
            injector
                .when_called(injectorpp::func!(
                    unsafe{} fn (OpenProcessToken)(
                        HANDLE,
                        windows::Win32::Security::TOKEN_ACCESS_MASK,
                        *mut HANDLE
                    ) -> windows::core::Result<()>
                ))
                .will_execute(injectorpp::fake!(
                    func_type: unsafe fn(
                        _process: HANDLE,
                        _access: windows::Win32::Security::TOKEN_ACCESS_MASK,
                        _token: *mut HANDLE
                    ) -> windows::core::Result<()>,
                    returns: Err(windows::core::Error::from(ERROR_ACCESS_DENIED))
                ));

            let result = is_elevated();
            assert!(result.is_err());
            if let Err(err) = result {
                assert_eq!(err.code(), ERROR_ACCESS_DENIED.into());
            }
        }

        #[test]
        fn test_get_token_information_failure() {
            let mut injector = InjectorPP::new();

            // Mock GetTokenInformation to return an error
            injector
                .when_called(injectorpp::func!(
                    unsafe{} fn (GetTokenInformation)(
                        HANDLE,
                        windows::Win32::Security::TOKEN_INFORMATION_CLASS,
                        Option<*mut std::ffi::c_void>,
                        u32,
                        *mut u32
                    ) -> windows::core::Result<()>
                ))
                .will_execute(injectorpp::fake!(
                    func_type: unsafe fn(
                        _token: HANDLE,
                        _class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
                        _info: Option<*mut std::ffi::c_void>,
                        _length: u32,
                        _return_length: *mut u32
                    ) -> windows::core::Result<()>,
                    returns: Err(windows::core::Error::from(ERROR_INVALID_HANDLE))
                ));

            let result = is_elevated();
            assert!(result.is_err());
            if let Err(err) = result {
                assert_eq!(err.code(), ERROR_INVALID_HANDLE.into());
            }
        }
    }

    // Integration tests that check actual privilege status
    mod integration_tests {
        use super::*;

        #[test]
        fn test_privilege_detection_integration() {
            // This test will succeed regardless of elevation status
            // It just ensures the function can execute without panicking
            match is_elevated() {
                Ok(elevated) => {
                    eprintln!("Process elevation status: {elevated}");
                    // No assertions - we just want to ensure it works
                }
                Err(e) => {
                    eprintln!("Failed to check elevation status: {e}");
                    // This is also acceptable - some systems may not support this check
                }
            }
        }
    }
}
