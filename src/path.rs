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
    Foundation,
    Storage::FileSystem::{self, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_DESCRIPTOR},
};

const LRU_CACHE_CAPACITY: usize = 4 * 1024; // 4K

/// Trait for entries that can be resolved to a file path.
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

/// Resolves file paths from file IDs on an NTFS/ReFS volume, optionally using an LRU cache for efficiency.
#[derive(Debug)]
pub struct PathResolver<'a> {
    volume: &'a Volume,
    dir_fid_path_cache: Option<LruCache<u64, (PathBuf, OsString)>>,
}

impl<'a> PathResolver<'a> {
    /// Create a new `PathResolver` for a given NTFS/ReFs volume.
    ///
    /// # Arguments
    /// * `volume` - Reference to the `Volume` struct representing the NTFS/ReFS volume.
    pub fn new(volume: &'a Volume) -> Self {
        PathResolver {
            volume,
            dir_fid_path_cache: None,
        }
    }

    /// Create a new `PathResolver` for a given NTFS/ReFs volume.
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

        full_path.push(format!("{drive_letter}:\\"));
    } else if let Some(mount_point) = &volume.mount_point {
        full_path.push(mount_point);
    }

    full_path.push(sub_path);
    Ok(full_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mft::MftEntry, volume::Volume};
    use std::{
        ffi::{OsString, c_void},
        mem, ptr,
    };
    use windows::Win32::{Foundation::HANDLE, System::Ioctl::USN_RECORD_V2};

    // Mock implementations of PathResolvableEntry
    #[derive(Debug)]
    struct MockEntry {
        fid: u64,
        parent_fid: u64,
        file_name: OsString,
        is_dir: bool,
    }

    impl PathResolvableEntry for MockEntry {
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
            self.is_dir
        }
    }

    fn create_mock_volume() -> Volume {
        Volume {
            handle: HANDLE(0x1234 as *mut c_void),
            drive_letter: Some('C'),
            mount_point: None,
        }
    }

    #[test]
    fn test_mft_entry_path_resolvable_trait() {
        let entry = MftEntry {
            usn: 0x1000,
            fid: 0x123456,
            parent_fid: 0x654321,
            file_name: OsString::from("test.txt"),
            file_attributes: 0,
        };

        assert_eq!(entry.fid(), 0x123456);
        assert_eq!(entry.parent_fid(), 0x654321);
        assert_eq!(entry.file_name(), &OsString::from("test.txt"));
        assert!(!entry.is_dir());
    }

    #[test]
    fn test_usn_entry_path_resolvable_trait() {
        // Create a mock USN_RECORD_V2 to generate a UsnEntry
        let file_name = "document.txt";
        let file_name_utf16: Vec<u16> = file_name.encode_utf16().collect();
        let file_name_len = file_name_utf16.len() * mem::size_of::<u16>();
        let base_size = mem::size_of::<USN_RECORD_V2>();
        let total_size = base_size + file_name_len;
        let aligned_size = (total_size + 7) & !7; // 8-byte align

        let mut buffer = vec![0u8; aligned_size];

        // Create USN_RECORD_V2 header
        let record = USN_RECORD_V2 {
            RecordLength: aligned_size as u32,
            MajorVersion: 2,
            MinorVersion: 0,
            FileReferenceNumber: 0x789ABC,
            ParentFileReferenceNumber: 0xDEF123,
            Usn: 0x2000,
            TimeStamp: 0x12345678ABCDEF01i64,
            Reason: 0x80000000, // USN_REASON_FILE_CREATE
            SourceInfo: 0,
            SecurityId: 0,
            FileAttributes: 0,
            FileNameLength: file_name_len as u16,
            FileNameOffset: mem::offset_of!(USN_RECORD_V2, FileName) as u16,
            FileName: [0; 1],
        };

        // Copy the record header (without the FileName part which we'll handle separately)
        unsafe {
            ptr::copy_nonoverlapping(
                &record as *const USN_RECORD_V2 as *const u8,
                buffer.as_mut_ptr(),
                base_size - mem::size_of::<u16>(), // Exclude the [u16; 1] FileName field
            );
        }

        // Copy the actual filename starting at the FileName offset
        unsafe {
            let filename_ptr = buffer
                .as_mut_ptr()
                .add(mem::offset_of!(USN_RECORD_V2, FileName));
            ptr::copy_nonoverlapping(
                file_name_utf16.as_ptr() as *const u8,
                filename_ptr,
                file_name_len,
            );
        }

        let record_ref = unsafe { &*(buffer.as_ptr() as *const USN_RECORD_V2) };

        let entry = crate::journal::UsnEntry::new(record_ref);

        assert_eq!(entry.fid(), 0x789ABC);
        assert_eq!(entry.parent_fid(), 0xDEF123);
        assert_eq!(entry.file_name(), &OsString::from("document.txt"));
        assert!(!entry.is_dir());
    }

    #[test]
    fn test_path_resolver_new() {
        let volume = create_mock_volume();
        let resolver = PathResolver::new(&volume);

        assert!(resolver.dir_fid_path_cache.is_none());
    }

    #[test]
    fn test_path_resolver_new_with_cache() {
        let volume = create_mock_volume();
        let resolver = PathResolver::new_with_cache(&volume);

        assert!(resolver.dir_fid_path_cache.is_some());
    }

    #[test]
    fn test_resolve_path_with_cache_hit() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new_with_cache(&volume);

        // Pre-populate cache
        let cached_path = std::path::PathBuf::from("C:\\Documents\\Folder");
        let cached_name = OsString::from("test.txt");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x123456, (cached_path.clone(), cached_name.clone()));
        }

        let entry = MockEntry {
            fid: 0x123456,
            parent_fid: 0x654321,
            file_name: cached_name,
            is_dir: false,
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path, cached_path);
    }

    #[test]
    fn test_resolve_path_with_cache_miss_parent_hit() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new_with_cache(&volume);

        // Pre-populate cache with parent directory
        let cached_parent_path = std::path::PathBuf::from("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x654321, (cached_parent_path.clone(), cached_parent_name));
        }

        let entry = MockEntry {
            fid: 0x123456,
            parent_fid: 0x654321,
            file_name: OsString::from("newfile.txt"),
            is_dir: false,
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.to_string_lossy(), "C:\\Documents\\newfile.txt");
    }

    #[test]
    fn test_resolve_path_with_cache_directory_caching() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new_with_cache(&volume);

        // Pre-populate cache with parent directory
        let cached_parent_path = std::path::PathBuf::from("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x654321, (cached_parent_path.clone(), cached_parent_name));
        }

        let entry = MockEntry {
            fid: 0x123456,
            parent_fid: 0x654321,
            file_name: OsString::from("NewFolder"),
            is_dir: true, // This is a directory, should be cached
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.to_string_lossy(), "C:\\Documents\\NewFolder");

        // Verify the directory was cached
        if let Some(ref cache) = resolver.dir_fid_path_cache {
            assert!(cache.peek(&0x123456).is_some());
            let (cached_path, cached_name) = cache.peek(&0x123456).unwrap();
            assert_eq!(cached_path, &path);
            assert_eq!(cached_name, &OsString::from("NewFolder"));
        }
    }

    #[test]
    fn test_resolve_path_with_cache_name_mismatch() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new_with_cache(&volume);

        // Pre-populate cache with old name
        let cached_path = std::path::PathBuf::from("C:\\Documents\\OldName");
        let cached_old_name = OsString::from("OldName");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x123456, (cached_path.clone(), cached_old_name));
        }

        // Pre-populate parent cache
        let cached_parent_path = std::path::PathBuf::from("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x654321, (cached_parent_path.clone(), cached_parent_name));
        }

        let entry = MockEntry {
            fid: 0x123456,
            parent_fid: 0x654321,
            file_name: OsString::from("NewName"), // Different name than cached
            is_dir: true,
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.to_string_lossy(), "C:\\Documents\\NewName");

        // Verify the cache was updated with the new name
        if let Some(ref cache) = resolver.dir_fid_path_cache {
            let (updated_path, updated_name) = cache.peek(&0x123456).unwrap();
            assert_eq!(updated_path.to_string_lossy(), "C:\\Documents\\NewName");
            assert_eq!(updated_name, &OsString::from("NewName"));
        }
    }

    #[test]
    fn test_lru_cache_capacity() {
        let volume = create_mock_volume();
        let resolver = PathResolver::new_with_cache(&volume);

        if let Some(ref cache) = resolver.dir_fid_path_cache {
            // LRU cache should have the expected capacity
            assert_eq!(cache.cap().get(), LRU_CACHE_CAPACITY);
        }
    }

    #[test]
    fn test_mock_entry_trait_implementation() {
        let entry = MockEntry {
            fid: 0xABC123,
            parent_fid: 0xDEF456,
            file_name: OsString::from("test_file.dat"),
            is_dir: true,
        };

        assert_eq!(entry.fid(), 0xABC123);
        assert_eq!(entry.parent_fid(), 0xDEF456);
        assert_eq!(entry.file_name(), &OsString::from("test_file.dat"));
        assert!(entry.is_dir());
    }

    #[test]
    fn test_resolve_path_failure() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new(&volume);

        let entry = MockEntry {
            fid: 0x123456,
            parent_fid: 0x654321,
            file_name: OsString::from("test.txt"),
            is_dir: false,
        };

        // Since we can't mock the Windows API calls without Injectorpp,
        // this test will naturally fail when trying to resolve paths
        // against non-existent file IDs
        let result = resolver.resolve_path(&entry);
        assert!(result.is_none());
    }
}
