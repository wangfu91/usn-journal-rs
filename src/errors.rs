//! This module defines the custom error types.

use thiserror::Error;

/// Custom error type for USN Journal and MFT operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UsnError {
    /// The operation requires permissions that the caller does not have.
    #[error("Access denied: appropriate permissions or Administrator privileges required")]
    PermissionError,

    /// The volume's USN change journal is not active.
    ///
    /// Returned by the Windows `UsnJournal::query` API when the volume has no
    /// active change journal. Call `UsnJournal::query_or_create`
    /// to create one on demand.
    #[error("The USN change journal is not active on this volume")]
    JournalNotActive,

    /// Caller-provided options failed validation.
    #[error("Invalid options: {0}")]
    InvalidOptions(&'static str),

    /// Parsed record bytes were structurally invalid.
    #[error("Invalid record data: {0}")]
    InvalidRecordData(&'static str),

    /// The reported byte count exceeded the size of the buffer we provided.
    #[error("bytes_read exceeds buffer size: bytes_read={bytes_read}, buffer_len={buffer_len}")]
    InvalidBytesRead {
        /// The byte count returned by the Windows API.
        bytes_read: usize,
        /// The actual length of the backing buffer.
        buffer_len: usize,
    },

    /// A record or cursor was truncated before all required bytes were available.
    #[error("Truncated record at offset {offset}: needed {needed} bytes, got {got}")]
    TruncatedRecord {
        /// Byte offset of the truncated structure within the buffer.
        offset: u64,
        /// Number of bytes needed to read the full structure.
        needed: usize,
        /// Number of bytes that were actually available.
        got: usize,
    },

    /// A USN record declared an invalid byte length.
    #[error("Invalid USN record length at offset {offset}: {length} bytes ({reason})")]
    InvalidRecordLength {
        /// Byte offset of the record header within the buffer.
        offset: u64,
        /// Record length reported by the record header.
        length: u32,
        /// Human-readable reason the length was rejected.
        reason: &'static str,
    },

    /// The parser encountered a USN record major version it does not support.
    #[error("Unsupported USN record version {major_version} at offset {offset}")]
    UnsupportedRecordVersion {
        /// Byte offset of the record header within the buffer.
        offset: u64,
        /// Unsupported major version reported by the record.
        major_version: u16,
    },

    /// A parsed record violated an alignment rule required by the Windows layout.
    #[error("Misaligned record at offset {offset}: {reason}")]
    MisalignedRecord {
        /// Byte offset of the misaligned structure within the buffer.
        offset: u64,
        /// Human-readable reason the alignment check failed.
        reason: &'static str,
    },

    /// A mount point path could not be resolved to a volume.
    #[error("Invalid mount point: {0}")]
    InvalidMountPointError(String),

    /// A timestamp could not be represented in the target format.
    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(&'static str),

    /// A standard Rust I/O error.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// A Win32 API call failed.
    #[error("Win32 API error: {0}")]
    #[cfg(windows)]
    WinApiError(#[from] windows::core::Error),

    /// A provided buffer was too small for the requested work.
    #[error("Buffer too small: needed {needed} bytes, got {got}")]
    BufferTooSmall {
        /// Number of bytes required.
        needed: usize,
        /// Number of bytes actually provided.
        got: usize,
    },

    /// A generic record-level validation error.
    #[error("Invalid record at offset {offset}: {reason}")]
    InvalidRecord {
        /// Byte offset of the rejected record.
        offset: u64,
        /// Human-readable reason the record was rejected.
        reason: &'static str,
    },
}

impl UsnError {
    /// Whether this is a permission failure, including an underlying OS error.
    #[must_use]
    pub fn is_permission_denied(&self) -> bool {
        match self {
            Self::PermissionError => true,
            Self::IoError(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
            #[cfg(windows)]
            Self::WinApiError(err) => {
                use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PRIVILEGE_NOT_HELD};
                err.code() == ERROR_ACCESS_DENIED.into()
                    || err.code() == ERROR_PRIVILEGE_NOT_HELD.into()
            }
            _ => false,
        }
    }

    /// Return `true` if this error came from operating-system I/O.
    #[must_use]
    pub const fn is_io_error(&self) -> bool {
        match self {
            Self::IoError(_) => true,
            #[cfg(windows)]
            Self::WinApiError(_) => true,
            _ => false,
        }
    }

    /// Return `true` if this error indicates malformed on-disk data.
    #[must_use]
    pub const fn is_parse_error(&self) -> bool {
        matches!(
            self,
            Self::InvalidRecordData(_)
                | Self::InvalidBytesRead { .. }
                | Self::TruncatedRecord { .. }
                | Self::InvalidRecordLength { .. }
                | Self::UnsupportedRecordVersion { .. }
                | Self::MisalignedRecord { .. }
                | Self::BufferTooSmall { .. }
                | Self::InvalidRecord { .. }
        )
    }
}

#[cfg(all(test, windows))]
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
                "Access denied: appropriate permissions or Administrator privileges required"
            );
        }

        #[test]
        fn test_journal_not_active_display_and_classification() {
            let error = UsnError::JournalNotActive;
            assert_eq!(
                error.to_string(),
                "The USN change journal is not active on this volume"
            );
            // It is a precondition error, neither I/O nor on-disk parse.
            assert!(!error.is_io_error());
            assert!(!error.is_parse_error());
        }

        #[test]
        fn test_buffer_too_small_display() {
            let error = UsnError::BufferTooSmall {
                needed: 1024,
                got: 256,
            };
            assert_eq!(
                error.to_string(),
                "Buffer too small: needed 1024 bytes, got 256"
            );
        }

        #[test]
        fn test_invalid_record_display() {
            let error = UsnError::InvalidRecord {
                offset: 4096,
                reason: "bad magic",
            };
            assert_eq!(
                error.to_string(),
                "Invalid record at offset 4096: bad magic"
            );
        }

        #[test]
        fn test_precise_record_errors_display() {
            assert_eq!(
                UsnError::InvalidBytesRead {
                    bytes_read: 16,
                    buffer_len: 8
                }
                .to_string(),
                "bytes_read exceeds buffer size: bytes_read=16, buffer_len=8"
            );
            assert_eq!(
                UsnError::TruncatedRecord {
                    offset: 8,
                    needed: 60,
                    got: 12
                }
                .to_string(),
                "Truncated record at offset 8: needed 60 bytes, got 12"
            );
            assert_eq!(
                UsnError::UnsupportedRecordVersion {
                    offset: 24,
                    major_version: 4
                }
                .to_string(),
                "Unsupported USN record version 4 at offset 24"
            );
        }

        #[test]
        fn test_error_classification_helpers() {
            assert!(UsnError::IoError(IoError::other("disk")).is_io_error());
            assert!(UsnError::WinApiError(ERROR_ACCESS_DENIED.into()).is_io_error());
            assert!(!UsnError::PermissionError.is_io_error());

            assert!(!UsnError::InvalidOptions("bad option").is_parse_error());
        }

        #[test]
        fn test_invalid_mount_point_error_display() {
            let mount_point = "C:\\invalid\\path";
            let error = UsnError::InvalidMountPointError(mount_point.to_string());
            let error_string = error.to_string();
            assert_eq!(error_string, "Invalid mount point: C:\\invalid\\path");
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
            assert!(error_string.contains("I/O error:"));
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
            assert!(error.to_string().contains("Administrator privileges"));
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

#[cfg(all(test, windows))]
mod permission_tests {
    use super::*;
    #[test]
    fn classifies_permissions_without_losing_os_source() {
        use std::error::Error;
        use windows::Win32::Foundation::{
            ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_PRIVILEGE_NOT_HELD,
        };
        for code in [ERROR_ACCESS_DENIED, ERROR_PRIVILEGE_NOT_HELD] {
            let error = UsnError::from(windows::core::Error::from(code));
            assert!(error.is_permission_denied());
            assert!(error.source().is_some());
            assert!(matches!(error, UsnError::WinApiError(e) if e.code() == code.into()));
        }
        assert!(UsnError::PermissionError.is_permission_denied());
        assert!(
            UsnError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                .is_permission_denied()
        );
        assert!(
            !UsnError::from(windows::core::Error::from(ERROR_INVALID_HANDLE))
                .is_permission_denied()
        );
    }
}
