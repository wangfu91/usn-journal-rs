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
    ffi::{OsString, c_void},
    num::NonZeroUsize,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
};
use windows::Win32::{
    Foundation::{self, HANDLE},
    Storage::FileSystem::{self, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_DESCRIPTOR},
};

/// Trait for types that can resolve paths for specific entry types.
pub trait PathResolveTrait {
    /// The type of entry this resolver can process (e.g., MftEntry, UsnEntry).
    type InputEntry;

    /// Resolve the full path for a given entry.
    ///
    /// # Arguments
    /// * `entry` - Reference to the entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    fn resolve_path(&mut self, entry: &Self::InputEntry) -> Option<PathBuf>;
}

const LRU_CACHE_CAPACITY: usize = 4 * 1024; // 4K

/// Resolves file paths from file IDs on an NTFS/ReFS volume, using an LRU cache for efficiency.
#[derive(Debug)]
struct PathResolver {
    volume_handle: HANDLE,                      // Handle to the NTFS/ReFS volume
    drive_letter: Option<char>,                 // Optional drive letter (e.g., 'C')
    dir_fid_path_cache: LruCache<u64, PathBuf>, // LRU cache for dir file ID to path mapping
}

/// Path resolver for MFT-based lookups.
#[derive(Debug)]
pub struct MftPathResolver {
    path_resolver: PathResolver,
}

impl MftPathResolver {
    /// Create a new `MftPathResolver` from an MFT reference using the default LRU cache capacity.
    ///
    /// # Arguments
    /// * `mft` - Reference to the MFT.
    pub fn new(mft: &Mft) -> Self {
        let path_resolver = PathResolver::new(&mft.volume, None);
        MftPathResolver { path_resolver }
    }

    /// Create a new `MftPathResolver` from an MFT reference with a specified LRU cache capacity.
    ///
    /// # Arguments
    /// * `mft` - Reference to the MFT.
    /// * `capacity` - The capacity for the LRU cache. Should be greater than 0. Default is 4K.
    pub fn with_capacity(mft: &Mft, capacity: usize) -> Self {
        let path_resolver = PathResolver::new(&mft.volume, Some(capacity));
        MftPathResolver { path_resolver }
    }
}

impl PathResolveTrait for MftPathResolver {
    type InputEntry = MftEntry;

    /// Resolve the full path for a given MFT entry.
    ///
    /// # Arguments
    /// * `mft_entry` - Reference to the MFT entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    fn resolve_path(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.path_resolver.resolve_path_for_mft(mft_entry)
    }
}

/// Path resolver for USN journal-based lookups.
#[derive(Debug)]
pub struct JournalPathResolver {
    path_resolver: PathResolver,
}

impl JournalPathResolver {
    /// Create a new `JournalPathResolver` from a USN journal reference using the default LRU cache capacity.
    ///
    /// # Arguments
    /// * `journal` - Reference to the USN journal.
    pub fn new(journal: &UsnJournal) -> Self {
        let path_resolver = PathResolver::new(&journal.volume, None);
        JournalPathResolver { path_resolver }
    }

    /// Create a new `JournalPathResolver` from a USN journal reference with a specified LRU cache capacity.
    ///
    /// # Arguments
    /// * `journal` - Reference to the USN journal.
    /// * `capacity` - The capacity for the LRU cache. Should be greater than 0. Default is 4K.
    pub fn with_capacity(journal: &UsnJournal, capacity: usize) -> Self {
        let path_resolver = PathResolver::new(&journal.volume, Some(capacity));
        JournalPathResolver { path_resolver }
    }
}

impl PathResolveTrait for JournalPathResolver {
    type InputEntry = UsnEntry;

    /// Resolve the full path for a given USN entry.
    ///
    /// # Arguments
    /// * `usn_entry` - Reference to the USN entry.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    fn resolve_path(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.path_resolver.resolve_path_for_usn(usn_entry)
    }
}

impl PathResolver {
    /// Create a new `PathResolver` for a given NTFS/ReFs volume and drive letter.
    ///
    /// # Arguments
    /// * `volume` - Reference to the `Volume` struct representing the NTFS/ReFS volume.
    /// * `capacity_opt` - Optional LRU cache capacity. If `None` or 0, uses default.
    fn new(volume: &Volume, capacity_opt: Option<usize>) -> Self {
        let capacity = capacity_opt
            .filter(|&cap| cap > 0) // Ensure capacity is positive
            .and_then(NonZeroUsize::new) // Convert to NonZeroUsize
            .unwrap_or_else(|| NonZeroUsize::new(LRU_CACHE_CAPACITY).unwrap()); // Default if None or 0

        let fid_path_cache = LruCache::new(capacity);
        PathResolver {
            volume_handle: volume.handle,
            drive_letter: volume.drive_letter,
            dir_fid_path_cache: fid_path_cache,
        }
    }

    /// Resolve the full path for a given MFT entry.
    ///
    /// Uses the LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    fn resolve_path_for_mft(&mut self, mft_entry: &MftEntry) -> Option<PathBuf> {
        self.resolve_path(
            mft_entry.fid,
            mft_entry.parent_fid,
            &mft_entry.file_name,
            mft_entry.is_dir(),
        )
    }

    /// Resolve the full path for a given USN entry.
    ///
    /// Uses the LRU cache to speed up repeated lookups.
    /// Returns `Some(PathBuf)` if the path can be resolved, or `None` if not found.
    fn resolve_path_for_usn(&mut self, usn_entry: &UsnEntry) -> Option<PathBuf> {
        self.resolve_path(
            usn_entry.fid,
            usn_entry.parent_fid,
            &usn_entry.file_name,
            usn_entry.is_dir(),
        )
    }

    /// Internal: Resolve the full path from file ID, parent file ID, and file name.
    ///
    /// # Arguments
    /// * `fid` - File ID of the target file.
    /// * `parent_fid` - File ID of the parent directory.
    /// * `file_name` - File or directory name.
    /// * `is_dir` - Indicates if the target is a directory.
    ///
    /// # Returns
    /// * `Some(PathBuf)` - The resolved path if found.
    /// * `None` - If the path cannot be resolved.
    fn resolve_path(
        &mut self,
        fid: u64,
        parent_fid: u64,
        file_name: &OsString,
        is_dir: bool,
    ) -> Option<PathBuf> {
        if let Some(path) = self.dir_fid_path_cache.get(&fid) {
            return Some(path.clone());
        }

        if let Some(parent_path) = self.dir_fid_path_cache.get(&parent_fid) {
            let path = parent_path.join(file_name);

            if is_dir {
                self.dir_fid_path_cache.put(fid, path.clone());
            }

            return Some(path);
        }

        // If parent fid not in cache, try to get parent path from file system
        if let Ok(parent_path) = self.file_id_to_path(parent_fid) {
            let path = parent_path.join(file_name);

            self.dir_fid_path_cache.put(parent_fid, parent_path);

            if is_dir {
                self.dir_fid_path_cache.put(fid, path.clone());
            }

            return Some(path);
        }

        None
    }

    /// Resolves a file ID to its full path on the specified NTFS/ReFS volume.
    fn file_id_to_path(&self, file_id: u64) -> windows::core::Result<PathBuf> {
        let file_id_desc = FILE_ID_DESCRIPTOR {
            Type: FileSystem::FileIdType,
            dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
            Anonymous: FileSystem::FILE_ID_DESCRIPTOR_0 {
                FileId: file_id.try_into()?,
            },
        };

        let file_handle = unsafe {
            FileSystem::OpenFileById(
                self.volume_handle,
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

        if let Some(drive_letter) = self.drive_letter {
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
}
