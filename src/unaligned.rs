//! Checked helpers for reading fixed-size values from byte buffers.

use std::{mem::size_of, ptr};

/// Read a `Copy` value from `bytes[offset..]` without requiring alignment.
///
/// This centralizes the bounds check and unsafe unaligned read used by raw
/// Windows and NTFS parsers, whose buffers are byte-oriented and may not be
/// naturally aligned for Rust references.
#[inline]
pub(crate) fn read_unaligned_at<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(size_of::<T>())?;
    if end > bytes.len() {
        return None;
    }

    // SAFETY: The checked range above guarantees the full `T` lies within
    // `bytes`. `read_unaligned` handles any pointer alignment.
    Some(unsafe { ptr::read_unaligned(bytes.as_ptr().add(offset) as *const T) })
}
