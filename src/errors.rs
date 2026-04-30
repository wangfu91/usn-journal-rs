//! This module defines the custom error types.

use thiserror::Error;

/// Custom error type for USN Journal and MFT operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UsnError {
    #[error("Access denied: Administrator privileges required.")]
    PermissionError,

    #[error("Invalid options: {0}")]
    InvalidOptions(&'static str),

    #[error("Invalid record data: {0}")]
    InvalidRecordData(&'static str),

    #[error("Invalid mount point: {0}")]
    InvalidMountPointError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Windows API error: {0}")]
    WinApiError(#[from] windows::core::Error),

    #[error("Other error: {0}")]
    OtherError(String),

    #[error("Invalid NTFS boot sector: {0}")]
    InvalidBootSector(&'static str),

    #[error("Invalid MFT record {number}: {reason}")]
    InvalidMftRecord {
        number: u64,
        reason: &'static str,
    },

    #[error("Update sequence array mismatch in MFT record {number}")]
    FixupMismatch { number: u64 },

    #[error("Invalid NTFS data run: {0}")]
    InvalidDataRun(&'static str),

    #[error("MFT attribute missing: {0}")]
    MftAttributeMissing(&'static str),

    #[error("Unsupported filesystem: {0}")]
    UnsupportedFilesystem(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error as IoError, ErrorKind};
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;

    // Unit tests for UsnError variants and behavior
    mod error_variant_tests {
        use super::*;

        #[test]
        fn test_invalid_options_error_display() {
            let error = UsnError::InvalidOptions("buffer_size must be greater than 0");
            let error_string = error.to_string();
            assert_eq!(
                error_string,
                "Invalid options: buffer_size must be greater than 0"
            );
        }

        #[test]
        fn test_invalid_record_data_error_display() {
            let error = UsnError::InvalidRecordData("record length exceeds bytes read");
            let error_string = error.to_string();
            assert_eq!(
                error_string,
                "Invalid record data: record length exceeds bytes read"
            );
        }

        #[test]
        fn test_permission_error_display() {
            let error = UsnError::PermissionError;
            let error_string = error.to_string();
            assert_eq!(
                error_string,
                "Access denied: Administrator privileges required."
            );
        }

        #[test]
        fn test_invalid_mount_point_error_display() {
            let mount_point = "C:\\invalid\\path";
            let error = UsnError::InvalidMountPointError(mount_point.to_string());
            let error_string = error.to_string();
            assert_eq!(error_string, "Invalid mount point: C:\\invalid\\path");
        }

        #[test]
        fn test_other_error_display() {
            let message = "Custom error message";
            let error = UsnError::OtherError(message.to_string());
            let error_string = error.to_string();
            assert_eq!(error_string, "Other error: Custom error message");
        }

        #[test]
        fn test_io_error_conversion() {
            let io_error = IoError::new(ErrorKind::NotFound, "File not found");
            let usn_error = UsnError::from(io_error);

            match usn_error {
                UsnError::IoError(ref e) => {
                    assert_eq!(e.kind(), ErrorKind::NotFound);
                    assert_eq!(e.to_string(), "File not found");
                }
                _ => panic!("Expected IoError variant"),
            }
        }

        #[test]
        fn test_windows_error_conversion() {
            let win_error = windows::core::Error::from(ERROR_ACCESS_DENIED);
            let usn_error = UsnError::from(win_error);

            match usn_error {
                UsnError::WinApiError(ref e) => {
                    assert_eq!(e.code(), ERROR_ACCESS_DENIED.into());
                }
                _ => panic!("Expected WinApiError variant"),
            }
        }

        #[test]
        fn test_error_chain_display() {
            let io_error = IoError::new(ErrorKind::PermissionDenied, "Access denied");
            let usn_error = UsnError::from(io_error);
            let error_string = usn_error.to_string();
            assert!(error_string.contains("IO error:"));
            assert!(error_string.contains("Access denied"));
        }
    }

    // Tests for error matching and handling patterns
    mod error_handling_tests {
        use super::*;

        #[test]
        fn test_result_type_integration() {
            // Test that UsnError works correctly with Result types
            fn returns_permission_error() -> Result<(), UsnError> {
                Err(UsnError::PermissionError)
            }

            fn returns_ok() -> Result<String, UsnError> {
                Ok("success".to_string())
            }

            assert!(returns_permission_error().is_err());
            assert!(returns_ok().is_ok());
            assert_eq!(returns_ok().unwrap(), "success");
        }

        #[test]
        fn test_error_source_chain() {
            use std::error::Error;

            let io_error = IoError::new(ErrorKind::NotFound, "Original error");
            let usn_error = UsnError::from(io_error);

            // Test that the source chain is preserved
            assert!(usn_error.source().is_some());
            if let UsnError::IoError(ref e) = usn_error {
                assert_eq!(e.to_string(), "Original error");
            }
        }
    }

    // Tests for specific error scenarios common in USN operations
    mod usn_specific_error_tests {
        use super::*;

        #[test]
        fn test_common_permission_scenarios() {
            // Test that permission errors have the expected message
            let error = UsnError::PermissionError;
            assert!(
                error
                    .to_string()
                    .contains("Administrator privileges required")
            );
        }

        #[test]
        fn test_mount_point_error_scenarios() {
            let invalid_paths = vec![
                "Z:\\nonexistent",
                "\\\\invalid\\unc\\path",
                "C:\\path\\that\\does\\not\\exist",
                "",
            ];

            for path in invalid_paths {
                let error = UsnError::InvalidMountPointError(path.to_string());
                assert!(error.to_string().contains("Invalid mount point:"));
                assert!(error.to_string().contains(path));
            }
        }

        #[test]
        fn test_windows_api_error_codes() {
            use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_INVALID_HANDLE};

            let error_codes = vec![
                ERROR_ACCESS_DENIED,
                ERROR_FILE_NOT_FOUND,
                ERROR_INVALID_HANDLE,
            ];

            for code in error_codes {
                let win_error = windows::core::Error::from(code);
                let usn_error = UsnError::from(win_error);

                if let UsnError::WinApiError(ref e) = usn_error {
                    assert_eq!(e.code(), code.into());
                } else {
                    panic!("Expected WinApiError variant");
                }
            }
        }
    }
}
