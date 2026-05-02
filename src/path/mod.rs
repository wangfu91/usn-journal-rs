//! Path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve full file paths from file IDs using MFT or USN journal data.

pub mod in_memory_tree;
pub use in_memory_tree::InMemoryDirTree;

use crate::{Fid, journal::UsnEntry, mft::MftEntry, raw_mft::RawMft, volume::Volume};
use lru::LruCache;
use std::{
    cell::RefCell,
    ffi::{OsString, c_void},
    num::NonZeroUsize,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use windows::Win32::{
    Foundation,
    Storage::FileSystem::{self, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_DESCRIPTOR},
};

/// NTFS root directory MFT record number (`$Root`).
///
/// In parent-chain walks, reaching this record means path resolution has
/// reached the filesystem root and should stop climbing.
pub(crate) const NTFS_ROOT_RECORD_NUMBER: u64 = 5;

/// LRU cache mapping a file ID to its `(full_path, leaf_name)` pair.
type DirLruCache = LruCache<Fid, (Arc<Path>, OsString)>;

/// Trait for entries that can be resolved to a file path.
pub trait PathResolvableEntry {
    fn fid(&self) -> Fid;
    fn parent_fid(&self) -> Fid;
    fn file_name(&self) -> &OsString;
    fn is_dir(&self) -> bool;
}

impl PathResolvableEntry for MftEntry {
    fn fid(&self) -> Fid {
        self.fid
    }
    fn parent_fid(&self) -> Fid {
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
    fn fid(&self) -> Fid {
        self.fid
    }
    fn parent_fid(&self) -> Fid {
        self.parent_fid
    }
    fn file_name(&self) -> &OsString {
        &self.file_name
    }
    fn is_dir(&self) -> bool {
        self.is_dir()
    }
}

/// Mask a standard 64-bit NTFS file reference number to its 48-bit record
/// number portion (clearing the 16-bit sequence number in the high bits).
pub(crate) fn mask_fid_to_record_number(fid: Fid) -> Option<u64> {
    fid.record_number()
}

/// Resolves file paths from file IDs on an NTFS/ReFS volume.
///
/// Use [`PathResolver::builder`] to configure and construct an instance:
///
/// ```no_run
/// use usn_journal_rs::{volume::Volume, path::PathResolver};
/// use std::num::NonZeroUsize;
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Default resolver — uncached, pure syscall resolution:
/// let resolver = PathResolver::builder(&volume).build();
///
/// // With an LRU directory cache for repeated lookups in the same directory:
/// let resolver = PathResolver::builder(&volume)
///     .with_lru_cache(NonZeroUsize::new(4_096).unwrap())
///     .build();
/// ```
///
/// `PathResolver` is intentionally `!Sync` — it carries an internal
/// scratch buffer (and optional in-memory tree) accessed via interior
/// mutability to keep the public `resolve_path` signature ergonomic.
#[derive(Debug)]
pub struct PathResolver<'a> {
    volume: &'a Volume,
    dir_fid_path_cache: Option<DirLruCache>,
    /// Reusable heap buffer for `GetFileInformationByHandleEx` calls.
    buffer: RefCell<Vec<u8>>,
    in_memory_tree: Option<InMemoryDirTree>,
}

impl<'a> PathResolver<'a> {
    /// Create a [`PathResolverBuilder`] for the given volume.
    ///
    /// The builder defaults to uncached syscall resolution. Use
    /// [`PathResolverBuilder::with_lru_cache`] to add a directory path cache, or
    /// [`PathResolverBuilder::build_with_in_memory_tree`] for the fastest NTFS
    /// full-scan strategy.
    #[must_use]
    pub fn builder(volume: &'a Volume) -> PathResolverBuilder<'a> {
        PathResolverBuilder::new(volume)
    }

    /// Resolve `entry` to a full path, using the in-memory tree (if
    /// configured), then the LRU cache (if configured), falling back to
    /// `OpenFileById` syscalls.
    ///
    /// Standard 64-bit NTFS IDs can use all resolver strategies. Extended
    /// 128-bit IDs (for example ReFS `USN_RECORD_V3` entries) skip the
    /// in-memory raw-`$MFT` tree and are resolved via `OpenFileById`.
    #[must_use]
    pub fn resolve_path<E: PathResolvableEntry>(&mut self, entry: &E) -> Option<PathBuf> {
        if let Some(tree) = &self.in_memory_tree
            && let Some(p) =
                tree.resolve_with_optional_drive(entry.fid(), self.volume.drive_letter())
        {
            return Some(p);
        }
        if let Some(cache) = &mut self.dir_fid_path_cache {
            resolve_path_with_cache(
                self.volume,
                entry.fid(),
                entry.parent_fid(),
                entry.file_name(),
                entry.is_dir(),
                cache,
                &self.buffer,
            )
        } else {
            resolve_path(
                self.volume,
                entry.fid(),
                entry.parent_fid(),
                entry.file_name(),
                &self.buffer,
            )
        }
    }
}

/// Builder for [`PathResolver`].
///
/// Obtain one via [`PathResolver::builder`].
///
/// # Example
/// ```no_run
/// use usn_journal_rs::{volume::Volume, raw_mft::RawMft, path::PathResolver};
/// use std::num::NonZeroUsize;
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Uncached resolver (the default):
/// let mut resolver = PathResolver::builder(&volume).build();
///
/// // With an LRU cache:
/// let mut resolver = PathResolver::builder(&volume)
///     .with_lru_cache(NonZeroUsize::new(4096).unwrap())
///     .build();
///
/// // With in-memory tree (NTFS only):
/// let raw_mft = RawMft::new(&volume).unwrap();
/// let mut resolver = PathResolver::builder(&volume)
///     .build_with_in_memory_tree(&raw_mft)
///     .unwrap();
/// ```
#[derive(Debug)]
pub struct PathResolverBuilder<'a> {
    volume: &'a Volume,
    lru_cache_capacity: Option<NonZeroUsize>,
}

impl<'a> PathResolverBuilder<'a> {
    fn new(volume: &'a Volume) -> Self {
        PathResolverBuilder {
            volume,
            lru_cache_capacity: None,
        }
    }

    /// Enable an LRU directory path cache with the given capacity.
    ///
    /// By default the builder produces an uncached resolver. Calling this
    /// method enables a cache that avoids repeated `OpenFileById` round-trips
    /// for files in the same directory.
    ///
    /// # Example
    /// ```no_run
    /// use usn_journal_rs::{volume::Volume, path::PathResolver};
    /// use std::num::NonZeroUsize;
    ///
    /// let volume = Volume::from_drive_letter('C').unwrap();
    /// let resolver = PathResolver::builder(&volume)
    ///     .with_lru_cache(NonZeroUsize::new(4096).unwrap())
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_lru_cache(mut self, capacity: NonZeroUsize) -> Self {
        self.lru_cache_capacity = Some(capacity);
        self
    }

    /// Build a [`PathResolver`] without an in-memory directory tree.
    #[must_use]
    pub fn build(self) -> PathResolver<'a> {
        PathResolver {
            volume: self.volume,
            dir_fid_path_cache: self.lru_cache_capacity.map(LruCache::new),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: None,
        }
    }

    /// Build a [`PathResolver`] backed by an in-memory directory tree built
    /// from the given raw `$MFT`.
    ///
    /// Path resolution checks the tree first; on a miss it falls back to
    /// `OpenFileById` syscalls (and caches the result when the LRU cache is
    /// enabled).
    ///
    /// Returns an error on non-NTFS volumes (e.g. ReFS) or if the MFT
    /// iteration fails.
    ///
    /// # Example
    /// ```no_run
    /// use usn_journal_rs::{volume::Volume, raw_mft::RawMft, path::PathResolver};
    ///
    /// let volume = Volume::from_drive_letter('C').unwrap();
    /// let raw_mft = RawMft::new(&volume).unwrap();
    /// let mut resolver = PathResolver::builder(&volume)
    ///     .build_with_in_memory_tree(&raw_mft)
    ///     .unwrap();
    /// for entry in raw_mft.try_iter().unwrap().flatten().take(100) {
    ///     if let Some(path) = resolver.resolve_path(&entry) {
    ///         println!("{}", path.display());
    ///     }
    /// }
    /// ```
    pub fn build_with_in_memory_tree(
        self,
        raw_mft: &RawMft<'_>,
    ) -> crate::UsnResult<PathResolver<'a>> {
        let tree = InMemoryDirTree::from_raw_mft(raw_mft)?;
        Ok(PathResolver {
            volume: self.volume,
            dir_fid_path_cache: self.lru_cache_capacity.map(LruCache::new),
            buffer: RefCell::new(Vec::new()),
            in_memory_tree: Some(tree),
        })
    }
}

fn resolve_path(
    volume: &Volume,
    fid: Fid,
    parent_fid: Fid,
    file_name: &OsString,
    buffer: &RefCell<Vec<u8>>,
) -> Option<PathBuf> {
    if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid, buffer) {
        return Some(resolved_parent_path.join(file_name));
    } else if let Ok(resolved_path) = file_id_to_path(volume, fid, buffer) {
        return Some(resolved_path);
    }

    None
}

/// Internal: Resolve the full path from file ID, parent file ID, and file name.
fn resolve_path_with_cache(
    volume: &Volume,
    fid: Fid,
    parent_fid: Fid,
    file_name: &OsString,
    is_dir: bool,
    cache: &mut DirLruCache,
    buffer: &RefCell<Vec<u8>>,
) -> Option<PathBuf> {
    // 1. Check cache for the current FID.
    if let Some((cached_path, cached_file_name)) = cache.get(&fid) {
        if cached_file_name == file_name {
            return Some(cached_path.to_path_buf());
        } else {
            cache.pop(&fid);
        }
    }

    // 2. Try to get the parent directory's path.
    let parent_dir_path: Arc<Path>;

    if let Some((cached_parent_path, _)) = cache.get(&parent_fid) {
        parent_dir_path = Arc::clone(cached_parent_path);
    } else if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid, buffer) {
        let parent_actual_name = resolved_parent_path
            .file_name()
            .map_or_else(OsString::new, |s| s.to_os_string());
        let arc_path: Arc<Path> = Arc::from(resolved_parent_path.as_path());
        cache.put(parent_fid, (Arc::clone(&arc_path), parent_actual_name));
        parent_dir_path = arc_path;
    } else {
        return None;
    }

    // 3. Construct the current item's path using the parent's path and the current file_name.
    let current_path = parent_dir_path.join(file_name);

    // 4. If the current item is a directory, cache its path and current name.
    if is_dir {
        let arc_current: Arc<Path> = Arc::from(current_path.as_path());
        cache.put(fid, (arc_current, file_name.clone()));
    }

    Some(current_path)
}

/// Resolves a file ID to its full path on the specified NTFS/ReFS volume.
fn file_id_to_path(
    volume: &Volume,
    file_id: Fid,
    buffer: &RefCell<Vec<u8>>,
) -> windows::core::Result<PathBuf> {
    let (id, id_type) = match file_id {
        Fid::Standard(id) => (
            FileSystem::FILE_ID_DESCRIPTOR_0 {
                FileId: i64::from_ne_bytes(id.to_ne_bytes()),
            },
            FileSystem::FileIdType,
        ),
        Fid::Extended(_) => (
            FileSystem::FILE_ID_DESCRIPTOR_0 {
                ExtendedFileId: crate::usn_record::fid_to_file_id_128(file_id)
                    .expect("extended fid branch must produce FILE_ID_128"),
            },
            FileSystem::ExtendedFileIdType,
        ),
    };

    let file_id_desc = FILE_ID_DESCRIPTOR {
        Type: id_type,
        dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
        Anonymous: id,
    };

    // SAFETY: `volume.handle` is a live volume handle owned by `volume`.
    // `&file_id_desc` is a stack-local that outlives the call. Returns
    // either an owned file handle or an error; ownership transfers to us.
    let file_handle = unsafe {
        FileSystem::OpenFileById(
            volume.handle,
            &file_id_desc,
            0,
            FileSystem::FILE_SHARE_READ
                | FileSystem::FILE_SHARE_WRITE
                | FileSystem::FILE_SHARE_DELETE,
            None,
            FILE_FLAGS_AND_ATTRIBUTES(FileSystem::FILE_FLAG_BACKUP_SEMANTICS.0),
        )?
    };

    let init_len = size_of::<u32>() + (Foundation::MAX_PATH as usize) * size_of::<u16>();
    // Reuse the per-resolver buffer to avoid reallocating per call.
    let mut info_buffer = buffer.borrow_mut();
    info_buffer.clear();
    info_buffer.resize(init_len, 0);

    loop {
        // SAFETY: `file_handle` is a live, owned handle from the
        // `OpenFileById` call above. `info_buffer` is a `Vec<u8>` of
        // exactly `info_buffer.len()` writable bytes; the FSCTL writes
        // a `FILE_NAME_INFO` into the front of that buffer.
        if let Err(err) = unsafe {
            FileSystem::GetFileInformationByHandleEx(
                file_handle,
                FileSystem::FileNameInfo,
                info_buffer.as_mut_ptr() as *mut c_void,
                info_buffer.len() as u32,
            )
        } {
            if err.code() == Foundation::ERROR_MORE_DATA.into() {
                // Long paths, needs to extend buffer size to hold it.
                // SAFETY: Even when `GetFileInformationByHandleEx` fails
                // with `ERROR_MORE_DATA`, it has written at least the
                // fixed-size prefix of `FILE_NAME_INFO` (the `u32`
                // `FileNameLength`). `read` performs an unaligned-safe
                // copy into a stack value — we only use `FileNameLength`
                // from the result, which has alignment 4 anyway.
                let name_info = unsafe {
                    std::ptr::read(info_buffer.as_ptr() as *const FileSystem::FILE_NAME_INFO)
                };

                let needed_len = name_info.FileNameLength + size_of::<u32>() as u32;
                info_buffer.resize(needed_len as usize, 0);
                continue;
            }

            // SAFETY: `file_handle` is the live, owned handle from
            // `OpenFileById` above; we are closing it exactly once on
            // the error path. We deliberately ignore any close error
            // here because we are already returning the original `err`.
            unsafe {
                let _ = Foundation::CloseHandle(file_handle);
            };
            return Err(err);
        }

        break;
    }

    // SAFETY: `file_handle` is the live, owned handle from
    // `OpenFileById`; this is the unique close on the success path.
    unsafe { Foundation::CloseHandle(file_handle) }?;
    // SAFETY: The successful `GetFileInformationByHandleEx` call above
    // wrote a `FILE_NAME_INFO` (sized to fit) into the front of
    // `info_buffer`. The buffer's lifetime covers `info`, and the start
    // of a `Vec<u8>`'s buffer satisfies any alignment requirement that
    // a packed Win32 struct made of `u32`/`u16` fields needs (the
    // allocator returns at least pointer-aligned memory).
    let info: &FileSystem::FILE_NAME_INFO =
        unsafe { &*(info_buffer.as_ptr() as *const FileSystem::FILE_NAME_INFO) };

    let name_len = info.FileNameLength as usize / size_of::<u16>();
    // SAFETY: `info` was filled by a successful FSCTL call, so its
    // `FileNameLength` reflects the true number of UTF-16 bytes
    // written into the trailing `FileName` array, which lives inside
    // `info_buffer`.
    let name_u16 = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
    let sub_path = OsString::from_wide(name_u16);

    // Create the full path directly with a single allocation
    let mut full_path = PathBuf::new();

    if let Some(drive_letter) = volume.drive_letter() {
        let drive_letter = if drive_letter.is_ascii_lowercase() {
            drive_letter.to_ascii_uppercase()
        } else {
            drive_letter
        };

        full_path.push(format!("{drive_letter}:\\"));
    } else if let Some(mount_point) = volume.mount_point() {
        full_path.push(mount_point);
    }

    full_path.push(sub_path);
    Ok(full_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mft::MftEntry, usn_record::UsnRecordRef, volume::Volume};
    use std::{ffi::OsString, mem, ptr};
    use windows::Win32::{Foundation::HANDLE, System::Ioctl::USN_RECORD_V2};

    // Mock implementations of PathResolvableEntry
    #[derive(Debug)]
    struct MockEntry {
        fid: Fid,
        parent_fid: Fid,
        file_name: OsString,
        is_dir: bool,
    }

    impl PathResolvableEntry for MockEntry {
        fn fid(&self) -> Fid {
            self.fid
        }
        fn parent_fid(&self) -> Fid {
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
        Volume::mock(
            HANDLE(std::ptr::null_mut()),
            crate::volume::VolumeSource::DriveLetter('C'),
        )
    }

    #[test]
    fn test_mft_entry_path_resolvable_trait() {
        let entry = MftEntry {
            usn: crate::Usn::new(0x1000),
            fid: Fid::new(0x123456),
            parent_fid: Fid::new(0x654321),
            file_name: OsString::from("test.txt"),
            file_attributes: 0,
        };

        assert_eq!(entry.fid(), Fid::new(0x123456));
        assert_eq!(entry.parent_fid(), Fid::new(0x654321));
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

        let entry = crate::journal::UsnEntry::new(UsnRecordRef::V2(record_ref));

        assert_eq!(entry.fid(), Fid::new(0x789ABC));
        assert_eq!(entry.parent_fid(), Fid::new(0xDEF123));
        assert_eq!(entry.file_name(), &OsString::from("document.txt"));
        assert!(!entry.is_dir());
    }

    fn arc_path(p: &str) -> Arc<Path> {
        Arc::from(Path::new(p))
    }

    #[test]
    fn test_resolve_path_with_cache_hit() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::builder(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap())
            .build();

        // Pre-populate cache
        let cached_path = arc_path("C:\\Documents\\Folder");
        let cached_name = OsString::from("test.txt");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(
                Fid::new(0x123456),
                (Arc::clone(&cached_path), cached_name.clone()),
            );
        }

        let entry = MockEntry {
            fid: Fid::new(0x123456),
            parent_fid: Fid::new(0x654321),
            file_name: cached_name,
            is_dir: false,
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.as_path(), &*cached_path);
    }

    #[test]
    fn test_resolve_path_with_cache_miss_parent_hit() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::builder(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap())
            .build();

        // Pre-populate cache with parent directory
        let cached_parent_path = arc_path("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(Fid::new(0x654321), (cached_parent_path, cached_parent_name));
        }

        let entry = MockEntry {
            fid: Fid::new(0x123456),
            parent_fid: Fid::new(0x654321),
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
        let mut resolver = PathResolver::builder(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap())
            .build();

        // Pre-populate cache with parent directory
        let cached_parent_path = arc_path("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(Fid::new(0x654321), (cached_parent_path, cached_parent_name));
        }

        let entry = MockEntry {
            fid: Fid::new(0x123456),
            parent_fid: Fid::new(0x654321),
            file_name: OsString::from("NewFolder"),
            is_dir: true, // This is a directory, should be cached
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.to_string_lossy(), "C:\\Documents\\NewFolder");

        // Verify the directory was cached
        if let Some(ref cache) = resolver.dir_fid_path_cache {
            assert!(cache.peek(&Fid::new(0x123456)).is_some());
            let (cached_path, cached_name) = cache.peek(&Fid::new(0x123456)).unwrap();
            assert_eq!(&**cached_path, path.as_path());
            assert_eq!(cached_name, &OsString::from("NewFolder"));
        }
    }

    #[test]
    fn test_resolve_path_with_cache_name_mismatch() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::builder(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap())
            .build();

        // Pre-populate cache with old name
        let cached_path = arc_path("C:\\Documents\\OldName");
        let cached_old_name = OsString::from("OldName");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(Fid::new(0x123456), (cached_path, cached_old_name));
        }

        // Pre-populate parent cache
        let cached_parent_path = arc_path("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(Fid::new(0x654321), (cached_parent_path, cached_parent_name));
        }

        let entry = MockEntry {
            fid: Fid::new(0x123456),
            parent_fid: Fid::new(0x654321),
            file_name: OsString::from("NewName"), // Different name than cached
            is_dir: true,
        };

        let result = resolver.resolve_path(&entry);
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.to_string_lossy(), "C:\\Documents\\NewName");

        // Verify the cache was updated with the new name
        if let Some(ref cache) = resolver.dir_fid_path_cache {
            let (updated_path, updated_name) = cache.peek(&Fid::new(0x123456)).unwrap();
            assert_eq!(updated_path.to_string_lossy(), "C:\\Documents\\NewName");
            assert_eq!(updated_name, &OsString::from("NewName"));
        }
    }

    #[test]
    fn test_resolve_path_failure() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::builder(&volume).build();

        let entry = MockEntry {
            fid: Fid::new(0x123456),
            parent_fid: Fid::new(0x654321),
            file_name: OsString::from("test.txt"),
            is_dir: false,
        };

        // Since we can't mock the Windows API calls without Injectorpp,
        // this test will naturally fail when trying to resolve paths
        // against non-existent file IDs
        let result = resolver.resolve_path(&entry);
        assert!(result.is_none());
    }

    // ------------------------------------------------------------------
    // Builder API tests
    // ------------------------------------------------------------------

    #[test]
    fn test_builder_default_has_no_cache_and_no_tree() {
        let volume = create_mock_volume();
        let resolver = PathResolver::builder(&volume).build();
        assert!(resolver.dir_fid_path_cache.is_none());
        assert!(resolver.in_memory_tree.is_none());
    }

    #[test]
    fn test_builder_with_lru_cache_sets_cache() {
        let volume = create_mock_volume();
        let cap = NonZeroUsize::new(64).unwrap();
        let resolver = PathResolver::builder(&volume).with_lru_cache(cap).build();
        assert!(resolver.dir_fid_path_cache.is_some());
        assert!(resolver.in_memory_tree.is_none());
    }

    #[test]
    fn test_builder_lru_cache_respects_capacity() {
        let volume = create_mock_volume();
        let cap = NonZeroUsize::new(8).unwrap();
        let resolver = PathResolver::builder(&volume).with_lru_cache(cap).build();
        let cache = resolver.dir_fid_path_cache.as_ref().unwrap();
        assert_eq!(cache.cap(), NonZeroUsize::new(8).unwrap());
    }

    #[test]
    fn test_builder_with_lru_cache_twice_keeps_last() {
        // Calling with_lru_cache twice should use the last capacity.
        let volume = create_mock_volume();
        let resolver = PathResolver::builder(&volume)
            .with_lru_cache(NonZeroUsize::new(32).unwrap())
            .with_lru_cache(NonZeroUsize::new(128).unwrap())
            .build();
        let cache = resolver.dir_fid_path_cache.as_ref().unwrap();
        assert_eq!(cache.cap(), NonZeroUsize::new(128).unwrap());
    }

    #[test]
    fn test_builder_in_memory_tree_empty_tree() {
        // Build an empty in-memory tree directly and verify it resolves nothing.
        let tree = InMemoryDirTree::default();
        assert!(tree.is_empty());
        assert!(tree.resolve(Fid::new(0xDEADBEEF)).is_none());
    }
}
