use super::*;

use crate::{Fid, mft::MftEntry, usn_record::UsnRecordView, volume::Volume};
use std::{ffi::OsString, mem, num::NonZeroUsize, path::Path, ptr, sync::Arc};
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
fn mft_entry_path_resolvable_trait() {
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
fn usn_entry_path_resolvable_trait() {
    let file_name = "document.txt";
    let file_name_utf16: Vec<u16> = file_name.encode_utf16().collect();
    let file_name_len = file_name_utf16.len() * mem::size_of::<u16>();
    let base_size = mem::size_of::<USN_RECORD_V2>();
    let total_size = base_size + file_name_len;
    let aligned_size = (total_size + 7) & !7;

    let mut buffer = vec![0u8; aligned_size];

    let record = USN_RECORD_V2 {
        RecordLength: aligned_size as u32,
        MajorVersion: 2,
        MinorVersion: 0,
        FileReferenceNumber: 0x789ABC,
        ParentFileReferenceNumber: 0xDEF123,
        Usn: 0x2000,
        TimeStamp: 0x12345678ABCDEF01i64,
        Reason: 0x80000000,
        SourceInfo: 0,
        SecurityId: 0,
        FileAttributes: 0,
        FileNameLength: file_name_len as u16,
        FileNameOffset: mem::offset_of!(USN_RECORD_V2, FileName) as u16,
        FileName: [0; 1],
    };

    unsafe {
        ptr::copy_nonoverlapping(
            &record as *const USN_RECORD_V2 as *const u8,
            buffer.as_mut_ptr(),
            base_size - mem::size_of::<u16>(),
        );
    }

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

    let entry = crate::journal::UsnEntry::new(UsnRecordView::V2(record_ref));

    assert_eq!(entry.fid(), Fid::new(0x789ABC));
    assert_eq!(entry.parent_fid(), Fid::new(0xDEF123));
    assert_eq!(entry.file_name(), &OsString::from("document.txt"));
    assert!(!entry.is_dir());
}

fn arc_path(p: &str) -> Arc<Path> {
    Arc::from(Path::new(p))
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn in_memory_tree_resolve_four_deep_path() {
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
fn in_memory_tree_cycle_detection() {
    let mut tree = InMemoryDirTree::default();
    // 10 -> 11 -> 10 (cycle)
    tree.insert(10, 11, &utf16("a"));
    tree.insert(11, 10, &utf16("b"));
    // The walker bounds at 256 steps; the chain visits 10, 11, 10, 11...
    // forever and returns None.
    assert!(tree.resolve(Fid::new(10)).is_none());
}

#[test]
fn in_memory_tree_missing_fid_returns_none() {
    let tree = InMemoryDirTree::default();
    assert!(tree.resolve(Fid::new(0xDEADBEEF)).is_none());
}

#[test]
fn in_memory_tree_fid_with_sequence_bits_is_masked() {
    let mut tree = InMemoryDirTree::default();
    tree.insert(10, 5, &utf16("hello"));
    // Lookup with the high 16 bits set (sequence number) must
    // still resolve to the same record.
    let fid = (0x0123u64 << 48) | 10;
    assert!(tree.resolve(Fid::new(fid)).is_some());
}

#[test]
fn resolve_path_with_cache_hit() {
    let volume = create_mock_volume();
    let mut resolver = PathResolver::new(&volume).with_lru_cache(NonZeroUsize::new(4096).unwrap());

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
    let path = result.expect("cache hit should resolve");
    assert_eq!(path.as_path(), &*cached_path);
}

#[test]
fn resolve_path_with_cache_miss_parent_hit() {
    let volume = create_mock_volume();
    let mut resolver = PathResolver::new(&volume).with_lru_cache(NonZeroUsize::new(4096).unwrap());

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
    let path = result.expect("parent cache hit should resolve");
    assert_eq!(path.to_string_lossy(), "C:\\Documents\\newfile.txt");
}

#[test]
fn resolve_path_with_cache_directory_caching() {
    let volume = create_mock_volume();
    let mut resolver = PathResolver::new(&volume).with_lru_cache(NonZeroUsize::new(4096).unwrap());

    let cached_parent_path = arc_path("C:\\Documents");
    let cached_parent_name = OsString::from("Documents");
    if let Some(ref mut cache) = resolver.dir_fid_path_cache {
        cache.put(Fid::new(0x654321), (cached_parent_path, cached_parent_name));
    }

    let entry = MockEntry {
        fid: Fid::new(0x123456),
        parent_fid: Fid::new(0x654321),
        file_name: OsString::from("NewFolder"),
        is_dir: true,
    };

    let result = resolver.resolve_path(&entry);
    assert!(result.is_some());
    let path = result.expect("directory should resolve");
    assert_eq!(path.to_string_lossy(), "C:\\Documents\\NewFolder");

    if let Some(ref cache) = resolver.dir_fid_path_cache {
        assert!(cache.peek(&Fid::new(0x123456)).is_some());
        let (cached_path, cached_name) = cache.peek(&Fid::new(0x123456)).unwrap();
        assert_eq!(&**cached_path, path.as_path());
        assert_eq!(cached_name, &OsString::from("NewFolder"));
    }
}

#[test]
fn resolve_path_with_cache_name_mismatch() {
    let volume = create_mock_volume();
    let mut resolver = PathResolver::new(&volume).with_lru_cache(NonZeroUsize::new(4096).unwrap());

    let cached_path = arc_path("C:\\Documents\\OldName");
    let cached_old_name = OsString::from("OldName");
    if let Some(ref mut cache) = resolver.dir_fid_path_cache {
        cache.put(Fid::new(0x123456), (cached_path, cached_old_name));
    }

    let cached_parent_path = arc_path("C:\\Documents");
    let cached_parent_name = OsString::from("Documents");
    if let Some(ref mut cache) = resolver.dir_fid_path_cache {
        cache.put(Fid::new(0x654321), (cached_parent_path, cached_parent_name));
    }

    let entry = MockEntry {
        fid: Fid::new(0x123456),
        parent_fid: Fid::new(0x654321),
        file_name: OsString::from("NewName"),
        is_dir: true,
    };

    let result = resolver.resolve_path(&entry);
    assert!(result.is_some());
    let path = result.expect("name mismatch should refresh cache");
    assert_eq!(path.to_string_lossy(), "C:\\Documents\\NewName");

    if let Some(ref cache) = resolver.dir_fid_path_cache {
        let (updated_path, updated_name) = cache.peek(&Fid::new(0x123456)).unwrap();
        assert_eq!(updated_path.to_string_lossy(), "C:\\Documents\\NewName");
        assert_eq!(updated_name, &OsString::from("NewName"));
    }
}

#[test]
fn resolve_path_failure() {
    let volume = create_mock_volume();
    let mut resolver = PathResolver::new(&volume).without_lru_cache();

    let entry = MockEntry {
        fid: Fid::new(0x123456),
        parent_fid: Fid::new(0x654321),
        file_name: OsString::from("test.txt"),
        is_dir: false,
    };

    let result = resolver.resolve_path(&entry);
    assert!(result.is_none());
}

#[test]
fn resolver_default_has_cache_and_no_tree() {
    let volume = create_mock_volume();
    let resolver = PathResolver::new(&volume);
    assert!(resolver.dir_fid_path_cache.is_some());
    assert!(resolver.in_memory_tree.is_none());
}

#[test]
fn resolver_without_lru_cache_disables_cache() {
    let volume = create_mock_volume();
    let resolver = PathResolver::new(&volume).without_lru_cache();
    assert!(resolver.dir_fid_path_cache.is_none());
    assert!(resolver.in_memory_tree.is_none());
}

#[test]
fn builder_with_lru_cache_sets_cache() {
    let volume = create_mock_volume();
    let cap = NonZeroUsize::new(64).unwrap();
    let resolver = PathResolver::new(&volume).with_lru_cache(cap);
    assert!(resolver.dir_fid_path_cache.is_some());
    assert!(resolver.in_memory_tree.is_none());
}

#[test]
fn builder_lru_cache_respects_capacity() {
    let volume = create_mock_volume();
    let cap = NonZeroUsize::new(8).unwrap();
    let resolver = PathResolver::new(&volume).with_lru_cache(cap);
    let cache = resolver.dir_fid_path_cache.as_ref().unwrap();
    assert_eq!(cache.cap(), NonZeroUsize::new(8).unwrap());
}

#[test]
fn builder_with_lru_cache_twice_keeps_last() {
    let volume = create_mock_volume();
    let resolver = PathResolver::new(&volume)
        .with_lru_cache(NonZeroUsize::new(32).unwrap())
        .with_lru_cache(NonZeroUsize::new(128).unwrap());
    let cache = resolver.dir_fid_path_cache.as_ref().unwrap();
    assert_eq!(cache.cap(), NonZeroUsize::new(128).unwrap());
}

#[test]
fn builder_in_memory_tree_empty_tree() {
    let tree = InMemoryDirTree::default();
    assert!(tree.is_empty());
    assert!(tree.resolve(Fid::new(0xDEADBEEF)).is_none());
}
