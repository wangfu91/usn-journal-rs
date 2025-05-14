use thiserror::Error;

#[derive(Debug, Error)]
pub enum UsnError {
    #[error("Access denied: Administrator privileges required. Please run the application as Administrator to access the USN journal.")]
    PermissionError,

    #[error("Invalid mount point")]
    InvalidMountPointError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Windows error: {0}")]
    WinApiError(#[from] windows::core::Error),

    #[error("Other error: {0}")]
    OtherError(String),
}
