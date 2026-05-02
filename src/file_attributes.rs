//! Internal helpers for decoding Windows file-attribute bitmasks.

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES,
};

/// Shared view over a raw Windows file-attribute bitmask.
pub(crate) trait FileAttributeView {
    /// Returns the underlying Windows file-attribute bitmask.
    fn raw_file_attributes(&self) -> u32;

    /// Returns true if the bitmask marks a directory.
    #[inline]
    fn has_directory_attribute(&self) -> bool {
        FILE_FLAGS_AND_ATTRIBUTES(self.raw_file_attributes()).contains(FILE_ATTRIBUTE_DIRECTORY)
    }

    /// Returns true if the bitmask marks a hidden item.
    #[inline]
    fn has_hidden_attribute(&self) -> bool {
        FILE_FLAGS_AND_ATTRIBUTES(self.raw_file_attributes()).contains(FILE_ATTRIBUTE_HIDDEN)
    }

    /// Strongly typed wrapper around the raw bitmask.
    #[inline]
    fn file_attribute_flags(&self) -> crate::FileAttributes {
        crate::FileAttributes::from_bits_retain(self.raw_file_attributes())
    }
}
