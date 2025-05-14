//! This module defines the custom error types.

use thiserror::Error;

/// Custom error type for USN Journal and MFT operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UsnError {
    #[error("Access denied: Administrator privileges required.")]
    PermissionError,

    #[error("Invalid mount point: {0}")]
    InvalidMountPointError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Windows API error: {0}")]
    WinApiError(#[from] windows::core::Error),

    #[error("Other error: {0}")]
    OtherError(String),
}
