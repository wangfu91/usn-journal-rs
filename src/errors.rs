//! This module defines the custom error types.

use thiserror::Error;

/// Custom error type for USN Journal and MFT operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UsnError {
    /// The process lacks the Administrator privileges required to open a volume.
    #[error("This operation requires Administrator privileges")]
    NotElevated,

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
    Io(#[from] std::io::Error),

    /// A Win32 API call failed.
    #[error("Win32 API error: {0}")]
    WinApi(#[from] windows::core::Error),

    /// The NTFS boot sector failed validation.
    #[error("Invalid NTFS boot sector: {0}")]
    InvalidBootSector(&'static str),

    /// An MFT record was invalid, but its volume offset is unknown.
    #[error("Invalid MFT record {number}: {reason}")]
    InvalidMftRecord {
        /// Record number in the `$MFT`.
        number: u64,
        /// Human-readable reason the record was rejected.
        reason: &'static str,
    },

    /// An MFT record was invalid and its volume offset is known.
    #[error("Invalid MFT record {number} at volume offset 0x{volume_offset:x}: {reason}")]
    InvalidMftRecordAt {
        /// Record number in the `$MFT`.
        number: u64,
        /// Byte offset of the record on disk.
        volume_offset: u64,
        /// Human-readable reason the record was rejected.
        reason: &'static str,
    },

    /// The update sequence array verification failed for an MFT record.
    #[error("Update sequence array mismatch in MFT record {number}")]
    FixupMismatch {
        /// Record number whose USA fixup failed validation.
        number: u64,
    },

    /// A runlist in a non-resident NTFS attribute was malformed.
    #[error("Invalid NTFS data run: {0}")]
    InvalidDataRun(&'static str),

    /// A required MFT attribute was missing from a record.
    #[error("MFT attribute missing: {0}")]
    MftAttributeMissing(&'static str),

    /// The target filesystem does not support the requested operation.
    #[error("Unsupported filesystem: {0}")]
    UnsupportedFilesystem(&'static str),

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
    /// Return `true` if this error came from operating-system I/O.
    #[must_use]
    pub const fn is_io_error(&self) -> bool {
        matches!(self, Self::Io(_) | Self::WinApi(_))
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
                | Self::InvalidBootSector(_)
                | Self::InvalidMftRecord { .. }
                | Self::InvalidMftRecordAt { .. }
                | Self::FixupMismatch { .. }
                | Self::InvalidDataRun(_)
                | Self::MftAttributeMissing(_)
                | Self::BufferTooSmall { .. }
                | Self::InvalidRecord { .. }
        )
    }

    /// Build the appropriate invalid-record variant based on whether a disk offset is known.
    pub(crate) fn invalid_mft_record(
        number: u64,
        volume_offset: Option<u64>,
        reason: &'static str,
    ) -> Self {
        match volume_offset {
            Some(volume_offset) => Self::InvalidMftRecordAt {
                number,
                volume_offset,
                reason,
            },
            None => Self::InvalidMftRecord { number, reason },
        }
    }
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
        fn test_not_elevated_display() {
            let error = UsnError::NotElevated;
            let error_string = error.to_string();
            assert_eq!(
                error_string,
                "This operation requires Administrator privileges"
            );
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
        fn test_invalid_mft_record_at_display() {
            let error = UsnError::invalid_mft_record(42, Some(0x1234), "bad header");
            assert_eq!(
                error.to_string(),
                "Invalid MFT record 42 at volume offset 0x1234: bad header"
            );
        }

        #[test]
        fn test_error_classification_helpers() {
            assert!(UsnError::Io(IoError::other("disk")).is_io_error());
            assert!(UsnError::WinApi(ERROR_ACCESS_DENIED.into()).is_io_error());
            assert!(!UsnError::NotElevated.is_io_error());

            assert!(UsnError::InvalidDataRun("bad run").is_parse_error());
            assert!(UsnError::FixupMismatch { number: 7 }.is_parse_error());
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
                UsnError::Io(ref e) => {
                    assert_eq!(e.kind(), ErrorKind::NotFound);
                    assert_eq!(e.to_string(), "File not found");
                }
                _ => panic!("Expected Io variant"),
            }
        }

        #[test]
        fn test_windows_error_conversion() {
            let win_error = windows::core::Error::from(ERROR_ACCESS_DENIED);
            let usn_error = UsnError::from(win_error);

            match usn_error {
                UsnError::WinApi(ref e) => {
                    assert_eq!(e.code(), ERROR_ACCESS_DENIED.into());
                }
                _ => panic!("Expected WinApi variant"),
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
                Err(UsnError::NotElevated)
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
            if let UsnError::Io(ref e) = usn_error {
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
            let error = UsnError::NotElevated;
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

                if let UsnError::WinApi(ref e) = usn_error {
                    assert_eq!(e.code(), code.into());
                } else {
                    panic!("Expected WinApi variant");
                }
            }
        }
    }
}
