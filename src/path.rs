//! Path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve full file paths from file IDs using MFT or USN journal data.

use crate::{journal::UsnEntry, mft::MftEntry, volume::Volume};
use lru::LruCache;
use std::{
    ffi::{OsString, c_void},
    num::NonZeroUsize,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
};
use windows::Win32::{
    Foundation::{self},
    Storage::FileSystem::{self, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_DESCRIPTOR},
};

pub trait PathResolvableEntry {
    fn fid(&self) -> u64;
    fn parent_fid(&self) -> u64;
    fn file_name(&self) -> &OsString;
    fn is_dir(&self) -> bool;
}

impl PathResolvableEntry for MftEntry {
    fn fid(&self) -> u64 {
        self.fid
    }
    fn parent_fid(&self) -> u64 {
        self.parent_fid
    }
    fn file_name(&self) -> &OsString {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_dir()
    }
}

impl PathResolvableEntry for UsnEntry {
    fn fid(&self) -> u64 {
        self.fid
    }
    fn parent_fid(&self) -> u64 {
        self.parent_fid
    }
    fn file_name(&self) -> &OsString {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_dir()
    }
}

const LRU_CACHE_CAPACITY: usize = 4 * 1024; // 4K

/// Resolves file paths from file IDs on an NTFS/ReFS volume, using an LRU cache for efficiency.
#[derive(Debug)]
pub struct PathResolver<'a> {
    volume: &'a Volume, // The NTFS/ReFS volume
    dir_fid_path_cache: Option<LruCache<u64, (PathBuf, OsString)>>, // LRU cache for dir file ID to (path, filename) mapping
}

impl<'a> PathResolver<'a> {
    /// Create a new `PathResolver` for a given NTFS/ReFs volume and drive letter.
    ///
    /// # Arguments
    /// * `volume` - Reference to the `Volume` struct representing the NTFS/ReFS volume.
    pub fn new(volume: &'a Volume) -> Self {
        PathResolver {
            volume,
            dir_fid_path_cache: None,
        }
    }

    /// Create a new `PathResolver` for a given NTFS/ReFs volume and drive letter.
    ///
    /// # Arguments
    /// * `volume` - Reference to the `Volume` struct representing the NTFS/ReFS volume.
    pub fn new_with_cache(volume: &'a Volume) -> Self {
        let capacity = NonZeroUsize::new(LRU_CACHE_CAPACITY).unwrap();
        let cache = LruCache::new(capacity);
        PathResolver {
            volume,
            dir_fid_path_cache: Some(cache),
        }
    }

    pub fn resolve_path<E: PathResolvableEntry>(&mut self, entry: &E) -> Option<PathBuf> {
        if let Some(cache) = &mut self.dir_fid_path_cache {
            resolve_path_with_cache(
                self.volume,
                entry.fid(),
                entry.parent_fid(),
                entry.file_name(),
                entry.is_dir(),
                cache,
            )
        } else {
            resolve_path(
                self.volume,
                entry.fid(),
                entry.parent_fid(),
                entry.file_name(),
            )
        }
    }
}

fn resolve_path(
    volume: &Volume,
    fid: u64,
    parent_fid: u64,
    file_name: &OsString,
) -> Option<PathBuf> {
    if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid) {
        return Some(resolved_parent_path.join(file_name));
    } else if let Ok(resolved_path) = file_id_to_path(volume, fid) {
        return Some(resolved_path);
    }

    None
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
fn resolve_path_with_cache(
    volume: &Volume,
    fid: u64,
    parent_fid: u64,
    file_name: &OsString,
    is_dir: bool,
    cache: &mut LruCache<u64, (PathBuf, OsString)>,
) -> Option<PathBuf> {
    // 1. Check cache for the current FID.
    if let Some((cached_path, cached_file_name)) = cache.get(&fid) {
        // If the FID is in cache, check if the filename matches the one used to create the cached path.
        if cached_file_name == file_name {
            // Names match. The cached path is valid for this FID with this name.
            return Some(cached_path.clone());
        } else {
            // Names differ. This means the directory (fid) was renamed since it was cached.
            // The cached_path is stale because its last component is the old name.
            // Remove it and proceed to re-resolve.
            cache.pop(&fid);
            // Fall through to re-resolve using parent information.
        }
    }

    // At this point, 'fid' is not in cache with the correct 'file_name',
    // or it wasn't in cache at all.

    // 2. Try to get the parent directory's path.
    let parent_dir_path: PathBuf;

    // 2a. Check cache for parent_fid.
    if let Some((cached_parent_path, _)) = cache.get(&parent_fid) {
        // We use the cached_parent_path. If the parent itself was renamed, this path might be
        // stale. However, this strategy prioritizes using the cache. The check for 'fid' above
        // handles if 'fid' itself was renamed. If this cached_parent_path leads to issues,
        // eventually the parent's entry might get updated when it's resolved directly.
        parent_dir_path = cached_parent_path.clone();
    }
    // 2b. Parent not in cache, resolve it from the file system.
    else if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid) {
        parent_dir_path = resolved_parent_path;
        // Cache this newly resolved parent path.
        // The name stored is the actual name of the parent directory as resolved.
        let parent_actual_name = parent_dir_path
            .file_name()
            .map_or_else(OsString::new, |s| s.to_os_string());
        cache.put(parent_fid, (parent_dir_path.clone(), parent_actual_name));
    }
    // 2c. Parent path could not be resolved.
    else {
        return None; // Cannot determine parent path.
    }

    // 3. Construct the current item's path using the parent's path and the current file_name.
    let current_path = parent_dir_path.join(file_name);

    // 4. If the current item is a directory, cache its path and current name.
    if is_dir {
        cache.put(fid, (current_path.clone(), file_name.clone()));
    }

    Some(current_path)
}

/// Resolves a file ID to its full path on the specified NTFS/ReFS volume.
fn file_id_to_path(volume: &Volume, file_id: u64) -> windows::core::Result<PathBuf> {
    let file_id_desc = FILE_ID_DESCRIPTOR {
        Type: FileSystem::FileIdType,
        dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
        Anonymous: FileSystem::FILE_ID_DESCRIPTOR_0 {
            FileId: file_id.try_into()?,
        },
    };

    let file_handle = unsafe {
        FileSystem::OpenFileById(
            volume.handle,
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

    if let Some(drive_letter) = volume.drive_letter {
        let drive_letter = if drive_letter.is_ascii_lowercase() {
            drive_letter.to_ascii_uppercase()
        } else {
            drive_letter
        };

        full_path.push(format!("{}:\\", drive_letter));
    } else if let Some(mount_point) = &volume.mount_point {
        full_path.push(mount_point);
    }

    full_path.push(sub_path);
    Ok(full_path)
}
