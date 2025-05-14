//! Path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve full file paths from file IDs using MFT or USN journal data.

use crate::{
    journal::{UsnEntry, UsnJournal},
    mft::{Mft, MftEntry},
    volume::Volume,
};
use lru::LruCache;
use std::{
    ffi::{c_void, OsString},
    num::NonZeroUsize,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
};
use windows::Win32::{
    Foundation::{self, HANDLE},
    Storage::FileSystem::{self, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_DESCRIPTOR},
};

const LRU_CACHE_CAPACITY: usize = 4 * 1024; // 4KB

/// Resolves file paths from file IDs on an NTFS/ReFS volume, using an LRU cache for efficiency.
#[derive(Debug)]
struct PathResolver {
    volume_handle: HANDLE,                  // Handle to the NTFS/ReFS volume
    drive_letter: Option<char>,             // Optional drive letter (e.g., 'C')
    fid_path_cache: LruCache<u64, PathBuf>, // LRU cache for file ID to path mapping
}

/// Path resolver for MFT-based lookups.
pub struct MftPathResolver {
    path_resolver: PathResolver,
}

impl MftPathResolver {
    /// Create a new `MftPathResolver` from an MFT reference.
    pub fn new(mft: &Mft) -> Self {
        let path_resolver = PathResolver::new(&mft.volume);
        MftPathResolver { path_resolver }
    }

    /// Resolve the full path for a given MFT entry.
    ///
    /// # Arguments
    /// * `mft_entry` - Reference to the MFT entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    pub fn resolve_path(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.path_resolver.resolve_path_from_mft(mft_entry)
    }
}

/// Path resolver for USN journal-based lookups.
pub struct JournalPathResolver {
    path_resolver: PathResolver,
}

impl JournalPathResolver {
    /// Create a new `UsnJournalPathResolver` from a USN journal reference.
    pub fn new(journal: &UsnJournal) -> Self {
        let path_resolver = PathResolver::new(&journal.volume);
        JournalPathResolver { path_resolver }
    }

    /// Resolve the full path for a given USN entry.
    ///
    /// # Arguments
    /// * `usn_entry` - Reference to the USN entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    pub fn resolve_path(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.path_resolver.resolve_path_from_usn(usn_entry)
    }
}

impl PathResolver {
    /// Create a new `PathResolver` for a given NTFS/ReFs volume and drive letter.
    ///
    /// # Arguments
    /// * `volume` - Reference to the `Volume` struct representing the NTFS/ReFS volume.
    fn new(volume: &Volume) -> Self {
        let fid_path_cache = LruCache::new(NonZeroUsize::new(LRU_CACHE_CAPACITY).unwrap());
        PathResolver {
            volume_handle: volume.handle,
            drive_letter: volume.drive_letter,
            fid_path_cache,
        }
    }

    /// Resolve the full path for a given MFT entry.
    ///
    /// Uses the LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    fn resolve_path_from_mft(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.resolve_path(mft_entry.fid, mft_entry.parent_fid, &mft_entry.file_name)
    }

    /// Resolve the full path for a given USN entry.
    ///
    /// Uses the LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    fn resolve_path_from_usn(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.resolve_path(usn_entry.fid, usn_entry.parent_fid, &usn_entry.file_name)
    }

    /// Internal: Resolve the full path from file ID, parent file ID, and file name.
    ///
    /// # Arguments
    /// * `fid` - File ID of the target file.
    /// * `parent_fid` - File ID of the parent directory.
    /// * `file_name` - File or directory name.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    fn resolve_path(&mut self, fid: u64, parent_fid: u64, file_name: &OsString) -> Option<PathBuf> {
        if let Some(path) = self.fid_path_cache.get(&fid) {
            return Some(path.clone());
        }

        if let Some(parent_path) = self.fid_path_cache.get(&parent_fid) {
            let path = parent_path.join(file_name);
            self.fid_path_cache.put(fid, path.clone());
            return Some(path);
        }

        // If not in cache, try to get parent path from file system
        if let Ok(parent_path) = file_id_to_path(self.volume_handle, self.drive_letter, parent_fid)
        {
            let path = parent_path.join(file_name);
            self.fid_path_cache.put(parent_fid, parent_path);
            self.fid_path_cache.put(fid, path.clone());
            return Some(path);
        }

        None
    }
}

/// Resolves a file ID to its full path on the specified NTFS/ReFS volume.
fn file_id_to_path(
    volume_handle: HANDLE,
    drive_letter: Option<char>,
    file_id: u64,
) -> windows::core::Result<PathBuf> {
    let file_id_desc = FILE_ID_DESCRIPTOR {
        Type: FileSystem::FileIdType,
        dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
        Anonymous: FileSystem::FILE_ID_DESCRIPTOR_0 {
            FileId: file_id.try_into()?,
        },
    };

    let file_handle = unsafe {
        FileSystem::OpenFileById(
            volume_handle,
            &file_id_desc,
            FileSystem::FILE_GENERIC_READ.0,
            FileSystem::FILE_SHARE_READ
                | FileSystem::FILE_SHARE_WRITE
                | FileSystem::FILE_SHARE_DELETE,
            None,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
        )?
    };

    let init_len = size_of::<u32>() + (Foundation::MAX_PATH as usize) * size_of::<u16>();
    let mut info_buffer = vec![0u8; init_len];

    loop {
        if let Err(err) = unsafe {
            FileSystem::GetFileInformationByHandleEx(
                file_handle,
                FileSystem::FileNameInfo,
                &mut *info_buffer as *mut _ as *mut c_void,
                info_buffer.len() as u32,
            )
        } {
            if err.code() == Foundation::ERROR_MORE_DATA.into() {
                // Long paths, needs to extend buffer size to hold it.
                let name_info = unsafe {
                    std::ptr::read(info_buffer.as_ptr() as *const FileSystem::FILE_NAME_INFO)
                };

                let needed_len = name_info.FileNameLength + size_of::<u32>() as u32;
                // expand info_buffer capacity to needed_len to hold the long path
                info_buffer.resize(needed_len as usize, 0);
                // try again
                continue;
            }

            return Err(err);
        }

        break;
    }

    unsafe { Foundation::CloseHandle(file_handle) }?;
    // SAFETY: The buffer is guaranteed to be large enough for FILE_NAME_INFO
    // and the pointer is valid for the lifetime of the buffer.
    let info: &FileSystem::FILE_NAME_INFO =
        unsafe { &*(info_buffer.as_ptr() as *const FileSystem::FILE_NAME_INFO) };

    let name_len = info.FileNameLength as usize / size_of::<u16>();
    let name_u16 = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
    let sub_path = OsString::from_wide(name_u16);

    // Create the full path directly with a single allocation
    let mut full_path = PathBuf::new();

    if let Some(drive_letter) = drive_letter {
        // Only convert to uppercase if it's lowercase
        let drive_letter = if drive_letter.is_ascii_lowercase() {
            drive_letter.to_ascii_uppercase()
        } else {
            drive_letter
        };

        full_path.push(format!("{}:\\", drive_letter));
    }

    full_path.push(sub_path);
    Ok(full_path)
}
