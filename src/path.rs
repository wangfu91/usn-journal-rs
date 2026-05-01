//! Path resolution utilities for NTFS/ReFS volumes.
//!
//! Provides types and logic to resolve full file paths from file IDs using MFT or USN journal data.

use crate::{Fid, journal::UsnEntry, mft::MftEntry, volume::Volume};
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

const LRU_CACHE_CAPACITY: usize = 4 * 1024; // 4K

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

/// Mask a 64-bit NTFS file reference number to its 48-bit record number
/// portion (clearing the 16-bit sequence number in the high bits).
pub(crate) fn mask_fid_to_record_number(fid: u64) -> u64 {
    fid & 0x0000_FFFF_FFFF_FFFF
}

/// Resolves file paths from file IDs on an NTFS/ReFS volume.
///
/// Use the builder methods to opt into performance features:
///
/// ```no_run
/// use usn_journal_rs::{volume::Volume, path::PathResolver};
/// use std::num::NonZeroUsize;
///
/// let volume = Volume::from_drive_letter('C').unwrap();
///
/// // Syscall-only (no cache):
/// let resolver = PathResolver::new(&volume);
///
/// // With LRU directory cache:
/// let resolver = PathResolver::new(&volume)
///     .with_lru_cache(NonZeroUsize::new(4096).unwrap());
/// ```
///
/// `PathResolver` is intentionally `!Sync` — it carries an internal
/// scratch buffer (and optional in-memory tree) accessed via interior
/// mutability to keep the public `resolve_path` signature ergonomic.
#[derive(Debug)]
pub struct PathResolver<'a> {
    volume: &'a Volume,
    dir_fid_path_cache: Option<LruCache<u64, (Arc<Path>, OsString)>>,
    scratch: RefCell<Vec<u8>>,
    in_memory_tree: Option<InMemoryDirTree>,
}

impl<'a> PathResolver<'a> {
    /// Create a new `PathResolver` for the given volume.
    ///
    /// By default the resolver uses `OpenFileById` syscalls with no cache.
    /// Chain [`with_lru_cache`][`Self::with_lru_cache`] or
    /// [`with_in_memory_tree`][`Self::with_in_memory_tree`] to enable
    /// faster resolution strategies.
    #[must_use]
    pub fn new(volume: &'a Volume) -> Self {
        PathResolver {
            volume,
            dir_fid_path_cache: None,
            scratch: RefCell::new(Vec::new()),
            in_memory_tree: None,
        }
    }

    /// Enable an LRU cache of the given capacity for directory path lookups.
    ///
    /// When enabled, resolved parent-directory paths are stored in the cache
    /// so that subsequent entries in the same directory avoid a
    /// `OpenFileById` round-trip.
    ///
    /// # Example
    /// ```no_run
    /// use usn_journal_rs::{volume::Volume, path::PathResolver};
    /// use std::num::NonZeroUsize;
    ///
    /// let volume = Volume::from_drive_letter('C').unwrap();
    /// let resolver = PathResolver::new(&volume)
    ///     .with_lru_cache(NonZeroUsize::new(4096).unwrap());
    /// ```
    #[must_use]
    pub fn with_lru_cache(mut self, capacity: NonZeroUsize) -> Self {
        self.dir_fid_path_cache = Some(LruCache::new(capacity));
        self
    }

    /// Create a `PathResolver` with a default-capacity LRU cache.
    ///
    /// # Deprecated
    /// Use [`PathResolver::new`] followed by
    /// [`.with_lru_cache`][`Self::with_lru_cache`] instead:
    ///
    /// ```no_run
    /// # use usn_journal_rs::{volume::Volume, path::PathResolver};
    /// # use std::num::NonZeroUsize;
    /// # let volume = Volume::from_drive_letter('C').unwrap();
    /// let resolver = PathResolver::new(&volume)
    ///     .with_lru_cache(NonZeroUsize::new(4096).unwrap());
    /// ```
    #[deprecated(
        since = "0.5.0",
        note = "Use `PathResolver::new(v).with_lru_cache(capacity)` instead"
    )]
    pub fn new_with_cache(volume: &'a Volume) -> Self {
        let capacity = NonZeroUsize::new(LRU_CACHE_CAPACITY)
            .expect("LRU_CACHE_CAPACITY must be greater than zero");
        Self::new(volume).with_lru_cache(capacity)
    }

    /// Build and attach an in-memory directory tree from the given raw `$MFT`.
    ///
    /// Path resolution will check the tree first; on a miss it falls back to
    /// `OpenFileById` syscalls (and caches the result if an LRU cache is also
    /// enabled via [`with_lru_cache`][`Self::with_lru_cache`]).
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
    /// let mut resolver = PathResolver::new(&volume)
    ///     .with_in_memory_tree(&raw_mft)
    ///     .unwrap();
    /// for entry in raw_mft.try_iter().unwrap().flatten().take(100) {
    ///     if let Some(path) = resolver.resolve_path(&entry) {
    ///         println!("{}", path.display());
    ///     }
    /// }
    /// ```
    pub fn with_in_memory_tree(
        mut self,
        raw_mft: &crate::raw_mft::RawMft<'_>,
    ) -> crate::UsnResult<Self> {
        let tree = InMemoryDirTree::from_raw_mft(raw_mft)?;
        self.in_memory_tree = Some(tree);
        Ok(self)
    }

    /// Resolve `entry` to a full path, using the in-memory tree (if
    /// configured), then the LRU cache (if configured), falling back to
    /// `OpenFileById` syscalls.
    #[must_use]
    pub fn resolve_path<E: PathResolvableEntry>(&mut self, entry: &E) -> Option<PathBuf> {
        if let Some(tree) = &self.in_memory_tree
            && let Some(p) = tree.resolve_with_optional_drive(entry.fid().get(), self.volume.drive_letter())
        {
            return Some(p);
        }
        if let Some(cache) = &mut self.dir_fid_path_cache {
            resolve_path_with_cache(
                self.volume,
                entry.fid().get(),
                entry.parent_fid().get(),
                entry.file_name(),
                entry.is_dir(),
                cache,
                &self.scratch,
            )
        } else {
            resolve_path(
                self.volume,
                entry.fid().get(),
                entry.parent_fid().get(),
                entry.file_name(),
                &self.scratch,
            )
        }
    }
}

fn resolve_path(
    volume: &Volume,
    fid: u64,
    parent_fid: u64,
    file_name: &OsString,
    scratch: &RefCell<Vec<u8>>,
) -> Option<PathBuf> {
    if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid, scratch) {
        return Some(resolved_parent_path.join(file_name));
    } else if let Ok(resolved_path) = file_id_to_path(volume, fid, scratch) {
        return Some(resolved_path);
    }

    None
}

/// Internal: Resolve the full path from file ID, parent file ID, and file name.
fn resolve_path_with_cache(
    volume: &Volume,
    fid: u64,
    parent_fid: u64,
    file_name: &OsString,
    is_dir: bool,
    cache: &mut LruCache<u64, (Arc<Path>, OsString)>,
    scratch: &RefCell<Vec<u8>>,
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
    } else if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid, scratch) {
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
    file_id: u64,
    scratch: &RefCell<Vec<u8>>,
) -> windows::core::Result<PathBuf> {
    let file_id_desc = FILE_ID_DESCRIPTOR {
        Type: FileSystem::FileIdType,
        dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
        Anonymous: FileSystem::FILE_ID_DESCRIPTOR_0 {
            FileId: file_id.try_into()?,
        },
    };

    // SAFETY: `volume.handle` is a live volume handle owned by `volume`.
    // `&file_id_desc` is a stack-local that outlives the call. Returns
    // either an owned file handle or an error; ownership transfers to us.
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
    // Reuse the per-resolver scratch buffer to avoid reallocating per call.
    let mut info_buffer = scratch.borrow_mut();
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

// =====================================================================
// In-memory directory tree
// =====================================================================

/// Directory entry in the in-memory tree. Stores the parent file
/// reference number (full 64-bit, not masked) and the leaf name as raw
/// UTF-16 units so we don't pay an `OsString` allocation per entry.
#[derive(Debug, Clone)]
struct DirEntry {
    parent: u64,
    name: Box<[u16]>,
}

/// Pre-built in-memory directory tree keyed by 48-bit MFT record number.
///
/// Built in a single pass over the raw `$MFT`. Resolving a path is then
/// a pointer chase up to the root with no syscalls and no `PathBuf`
/// allocations until the final assembly.
#[derive(Debug, Default, Clone)]
pub struct InMemoryDirTree {
    entries: std::collections::HashMap<u64, DirEntry>,
}

impl InMemoryDirTree {
    /// Build the tree from a raw `$MFT` reader. Iterates every record
    /// once. Skips entries marked unused in the `$MFT $BITMAP`.
    pub fn from_raw_mft(raw_mft: &crate::raw_mft::RawMft<'_>) -> crate::UsnResult<Self> {
        use std::os::windows::ffi::OsStrExt;
        let mut entries =
            std::collections::HashMap::with_capacity(raw_mft.record_count() as usize);
        for r in raw_mft.try_iter()? {
            let entry = match r {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.is_used {
                continue;
            }
            if entry.file_name.is_empty() {
                continue;
            }
            let key = mask_fid_to_record_number(entry.file_reference.get());
            // Encode the file name as raw UTF-16 once and store it.
            let units: Vec<u16> = entry.file_name.encode_wide().collect();
            entries.insert(
                key,
                DirEntry {
                    parent: entry.parent_reference.get(),
                    name: units.into_boxed_slice(),
                },
            );
        }
        Ok(InMemoryDirTree { entries })
    }

    /// Number of entries currently stored.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the tree has no entries.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert a directory entry (testing / advanced use).
    #[doc(hidden)]
    pub fn insert(&mut self, fid: u64, parent: u64, name: &[u16]) {
        self.entries.insert(
            mask_fid_to_record_number(fid),
            DirEntry {
                parent,
                name: name.to_vec().into_boxed_slice(),
            },
        );
    }

    /// Walks parents up to the root and returns the resolved path
    /// (without drive prefix). Returns `None` if the chain breaks or a
    /// cycle is detected.
    #[must_use]
    pub fn resolve(&self, fid: Fid) -> Option<PathBuf> {
        self.resolve_with_optional_drive(fid.get(), None)
    }

    /// Walks parents and prepends `<drive>:\` to the resolved path.
    #[must_use]
    pub fn resolve_with_drive_letter(&self, fid: Fid, drive: char) -> Option<PathBuf> {
        self.resolve_with_optional_drive(fid.get(), Some(drive))
    }

    fn resolve_with_optional_drive(&self, fid: u64, drive: Option<char>) -> Option<PathBuf> {
        // Maximum walk depth — far above the practical NTFS path-component
        // limit (~64 segments) and below any realistic cycle length.
        const MAX_STEPS: usize = 256;

        let mut chain: Vec<&[u16]> = Vec::with_capacity(32);
        let mut current = mask_fid_to_record_number(fid);
        let mut steps = 0usize;
        loop {
            if steps >= MAX_STEPS {
                return None;
            }
            steps += 1;

            let entry = self.entries.get(&current)?;
            chain.push(&entry.name);

            let parent = mask_fid_to_record_number(entry.parent);
            // NTFS root directory has record number 5 and self-references.
            if parent == current || parent == 5 {
                break;
            }
            current = parent;
        }

        let mut path = PathBuf::new();
        if let Some(drive) = drive {
            let drive = drive.to_ascii_uppercase();
            path.push(format!("{drive}:\\"));
        }
        for units in chain.iter().rev() {
            path.push(OsString::from_wide(units));
        }
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mft::MftEntry, volume::Volume};
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
        Volume::mock(HANDLE(std::ptr::null_mut()), crate::volume::VolumeSource::DriveLetter('C'))
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

        let entry = crate::journal::UsnEntry::new(record_ref);

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
        let mut resolver = PathResolver::new(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap());

        // Pre-populate cache
        let cached_path = arc_path("C:\\Documents\\Folder");
        let cached_name = OsString::from("test.txt");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x123456, (Arc::clone(&cached_path), cached_name.clone()));
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
        let mut resolver = PathResolver::new(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap());

        // Pre-populate cache with parent directory
        let cached_parent_path = arc_path("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x654321, (cached_parent_path, cached_parent_name));
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
        let mut resolver = PathResolver::new(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap());

        // Pre-populate cache with parent directory
        let cached_parent_path = arc_path("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x654321, (cached_parent_path, cached_parent_name));
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
            assert!(cache.peek(&0x123456).is_some());
            let (cached_path, cached_name) = cache.peek(&0x123456).unwrap();
            assert_eq!(&**cached_path, path.as_path());
            assert_eq!(cached_name, &OsString::from("NewFolder"));
        }
    }

    #[test]
    fn test_resolve_path_with_cache_name_mismatch() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new(&volume)
            .with_lru_cache(NonZeroUsize::new(4096).unwrap());

        // Pre-populate cache with old name
        let cached_path = arc_path("C:\\Documents\\OldName");
        let cached_old_name = OsString::from("OldName");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x123456, (cached_path, cached_old_name));
        }

        // Pre-populate parent cache
        let cached_parent_path = arc_path("C:\\Documents");
        let cached_parent_name = OsString::from("Documents");
        if let Some(ref mut cache) = resolver.dir_fid_path_cache {
            cache.put(0x654321, (cached_parent_path, cached_parent_name));
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
            let (updated_path, updated_name) = cache.peek(&0x123456).unwrap();
            assert_eq!(updated_path.to_string_lossy(), "C:\\Documents\\NewName");
            assert_eq!(updated_name, &OsString::from("NewName"));
        }
    }

    #[test]
    fn test_resolve_path_failure() {
        let volume = create_mock_volume();
        let mut resolver = PathResolver::new(&volume);

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
        let resolver = PathResolver::new(&volume);
        assert!(resolver.dir_fid_path_cache.is_none());
        assert!(resolver.in_memory_tree.is_none());
    }

    #[test]
    fn test_builder_with_lru_cache_sets_cache() {
        let volume = create_mock_volume();
        let cap = NonZeroUsize::new(64).unwrap();
        let resolver = PathResolver::new(&volume).with_lru_cache(cap);
        assert!(resolver.dir_fid_path_cache.is_some());
        assert!(resolver.in_memory_tree.is_none());
    }

    #[test]
    fn test_builder_lru_cache_respects_capacity() {
        let volume = create_mock_volume();
        let cap = NonZeroUsize::new(8).unwrap();
        let resolver = PathResolver::new(&volume).with_lru_cache(cap);
        let cache = resolver.dir_fid_path_cache.as_ref().unwrap();
        assert_eq!(cache.cap(), NonZeroUsize::new(8).unwrap());
    }

    #[test]
    fn test_builder_with_lru_cache_twice_keeps_last() {
        // Calling with_lru_cache twice should use the last capacity.
        let volume = create_mock_volume();
        let resolver = PathResolver::new(&volume)
            .with_lru_cache(NonZeroUsize::new(32).unwrap())
            .with_lru_cache(NonZeroUsize::new(128).unwrap());
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

    mod in_memory_tree_tests {
        use super::*;

        fn utf16(s: &str) -> Vec<u16> {
            s.encode_utf16().collect()
        }

        #[test]
        fn resolve_four_deep_path() {
            // Layout:
            //   5 (root)  -> "Users" (10) -> "alice" (20) -> "docs" (30) -> "todo.txt" (40)
            let mut tree = InMemoryDirTree::default();
            tree.insert(10, 5, &utf16("Users"));
            tree.insert(20, 10, &utf16("alice"));
            tree.insert(30, 20, &utf16("docs"));
            tree.insert(40, 30, &utf16("todo.txt"));

            let p = tree.resolve(Fid::new(40)).expect("resolved");
            // Without drive prefix, components join with the platform separator.
            assert_eq!(
                p.to_string_lossy().replace('/', "\\"),
                "Users\\alice\\docs\\todo.txt"
            );

            let p = tree
                .resolve_with_drive_letter(Fid::new(40), 'c')
                .expect("with drive");
            assert_eq!(p.to_string_lossy(), "C:\\Users\\alice\\docs\\todo.txt");
        }

        #[test]
        fn cycle_detection() {
            let mut tree = InMemoryDirTree::default();
            // 10 -> 11 -> 10 (cycle)
            tree.insert(10, 11, &utf16("a"));
            tree.insert(11, 10, &utf16("b"));
            // The walker bounds at 256 steps; the chain visits 10, 11, 10, 11...
            // forever and returns None.
            assert!(tree.resolve(Fid::new(10)).is_none());
        }

        #[test]
        fn missing_fid_returns_none() {
            let tree = InMemoryDirTree::default();
            assert!(tree.resolve(Fid::new(0xDEADBEEF)).is_none());
        }

        #[test]
        fn fid_with_sequence_bits_is_masked() {
            let mut tree = InMemoryDirTree::default();
            tree.insert(10, 5, &utf16("hello"));
            // Lookup with the high 16 bits set (sequence number) must
            // still resolve to the same record.
            let fid = (0x0123u64 << 48) | 10;
            assert!(tree.resolve(Fid::new(fid)).is_some());
        }
    }
}
