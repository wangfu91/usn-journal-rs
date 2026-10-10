//! Internal path-resolution helpers.

use lru::LruCache;
use std::{
    cell::RefCell,
    ffi::{OsStr, OsString, c_void},
    mem::size_of,
    os::windows::ffi::OsStringExt,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use windows::Win32::{
    Foundation,
    Storage::FileSystem::{self, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_DESCRIPTOR},
};

use crate::{Fid, volume::Volume};
use windows::core::Owned;

/// LRU cache mapping a file ID to its `(full_path, leaf_name)` pair.
pub(super) type DirLruCache = LruCache<Fid, (Arc<Path>, OsString)>;

/// Resolve a path without using the directory cache.
pub(crate) fn resolve_path(
    volume: &Volume,
    fid: Fid,
    parent_fid: Fid,
    file_name: &OsStr,
    buffer: &RefCell<Vec<u8>>,
) -> windows::core::Result<PathBuf> {
    if let Ok(resolved_parent_path) = file_id_to_path(volume, parent_fid, buffer) {
        return Ok(join_resolved_path(
            &resolved_parent_path,
            fid,
            parent_fid,
            file_name,
        ));
    }

    file_id_to_path(volume, fid, buffer)
}

/// Resolve a path using a shared parent-directory cache when possible.
pub(super) fn resolve_path_with_cache(
    volume: &Volume,
    fid: Fid,
    parent_fid: Fid,
    file_name: &OsStr,
    is_dir: bool,
    cache: &mut DirLruCache,
    buffer: &RefCell<Vec<u8>>,
) -> windows::core::Result<PathBuf> {
    // 1. Check cache for the current FID.
    if let Some((cached_path, cached_file_name)) = cache.get(&fid) {
        if cached_file_name.as_os_str() == file_name {
            return Ok(cached_path.to_path_buf());
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
        return file_id_to_path(volume, fid, buffer);
    }

    // 3. Construct the current item's path using the parent's path and the current file_name.
    let current_path = join_resolved_path(&parent_dir_path, fid, parent_fid, file_name);

    // 4. If the current item is a directory, cache its path and current name.
    if is_dir {
        let arc_current: Arc<Path> = Arc::from(current_path.as_path());
        cache.put(fid, (arc_current, file_name.to_os_string()));
    }

    Ok(current_path)
}

fn join_resolved_path(
    parent_dir_path: &Path,
    fid: Fid,
    parent_fid: Fid,
    file_name: &OsStr,
) -> PathBuf {
    // NTFS can surface the volume root as a self-entry in USN/MFT data where
    // `fid == parent_fid` and `file_name == "."`.
    //
    // `fsutil file queryfilenamebyid <drive> 0x...` confirms this FID resolves
    // to the volume root (for example `\\?\G:\`). If we join that root path
    // with a literal `.` component, the cache stores `G:\.` and descendants are
    // later reconstructed as `G:\.\foo`. Treat the self-entry as the already
    // resolved root path instead.
    if fid == parent_fid && file_name == OsStr::new(".") {
        parent_dir_path.to_path_buf()
    } else {
        parent_dir_path.join(file_name)
    }
}

fn push_volume_relative_path(base_path: &mut PathBuf, volume_relative_path: &Path) {
    let mut components = volume_relative_path.components();
    if matches!(components.next(), Some(Component::RootDir)) {
        base_path.push(components.as_path());
    } else {
        base_path.push(volume_relative_path);
    }
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
        Fid::Extended(id) => (
            FileSystem::FILE_ID_DESCRIPTOR_0 {
                ExtendedFileId: FileSystem::FILE_ID_128 {
                    Identifier: id.to_le_bytes(),
                },
            },
            FileSystem::ExtendedFileIdType,
        ),
    };

    let file_id_desc = FILE_ID_DESCRIPTOR {
        Type: id_type,
        dwSize: size_of::<FileSystem::FILE_ID_DESCRIPTOR>() as u32,
        Anonymous: id,
    };

    let file_handle = unsafe {
        Owned::new(FileSystem::OpenFileById(
            volume.handle(),
            &file_id_desc,
            FileSystem::FILE_GENERIC_READ.0,
            FileSystem::FILE_SHARE_READ
                | FileSystem::FILE_SHARE_WRITE
                | FileSystem::FILE_SHARE_DELETE,
            None,
            FILE_FLAG_BACKUP_SEMANTICS,
        )?)
    };

    let init_len = size_of::<u32>() + (Foundation::MAX_PATH as usize) * size_of::<u16>();
    let mut info_buffer = buffer.borrow_mut();
    if info_buffer.len() < init_len {
        info_buffer.resize(init_len, 0);
    }

    loop {
        if let Err(err) = unsafe {
            FileSystem::GetFileInformationByHandleEx(
                *file_handle,
                FileSystem::FileNameInfo,
                info_buffer.as_mut_ptr() as *mut c_void,
                info_buffer.len() as u32,
            )
        } {
            if err.code() == Foundation::ERROR_MORE_DATA.into() {
                // Long paths, needs to extend buffer size to hold it.
                let name_len = read_u32_le(&info_buffer, 0).ok_or_else(|| {
                    windows::core::Error::new(
                        Foundation::ERROR_INVALID_DATA.to_hresult(),
                        "Invalid FILE_NAME_INFO header",
                    )
                })?;

                let needed_len =
                    name_len
                        .checked_add(size_of::<u32>() as u32)
                        .ok_or_else(|| {
                            windows::core::Error::new(
                                Foundation::ERROR_INVALID_DATA.to_hresult(),
                                "FILE_NAME_INFO length overflow",
                            )
                        })?;
                // expand info_buffer capacity to needed_len to hold the long path
                info_buffer.resize(needed_len as usize, 0);
                // try again
                continue;
            }

            return Err(err);
        }

        break;
    }
    let file_name_len_bytes = read_u32_le(&info_buffer, 0).ok_or_else(|| {
        windows::core::Error::new(
            Foundation::ERROR_INVALID_DATA.to_hresult(),
            "Invalid FILE_NAME_INFO header",
        )
    })? as usize;
    if !file_name_len_bytes.is_multiple_of(size_of::<u16>()) {
        return Err(windows::core::Error::new(
            Foundation::ERROR_INVALID_DATA.to_hresult(),
            "Invalid UTF-16 file name length",
        ));
    }
    let name_start = size_of::<u32>();
    let name_end = name_start.checked_add(file_name_len_bytes).ok_or_else(|| {
        windows::core::Error::new(
            Foundation::ERROR_INVALID_DATA.to_hresult(),
            "FILE_NAME_INFO length overflow",
        )
    })?;
    let name_bytes = info_buffer.get(name_start..name_end).ok_or_else(|| {
        windows::core::Error::new(
            Foundation::ERROR_INVALID_DATA.to_hresult(),
            "FILE_NAME_INFO buffer too short",
        )
    })?;
    let mut name_u16 = Vec::with_capacity(file_name_len_bytes / 2);
    for chunk in name_bytes.as_chunks::<2>().0 {
        name_u16.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    let sub_path = OsString::from_wide(&name_u16);

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

    push_volume_relative_path(&mut full_path, Path::new(&sub_path));
    Ok(full_path)
}

fn read_u32_le(buffer: &[u8], offset: usize) -> Option<u32> {
    let bytes = buffer.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    #[test]
    fn test_join_resolved_path_keeps_root_self_entry_at_volume_root() {
        let path = join_resolved_path(
            Path::new(r"C:\"),
            Fid::new(0x5),
            Fid::new(0x5),
            OsStr::new("."),
        );

        assert_eq!(path, PathBuf::from(r"C:\"));
    }

    #[test]
    fn test_push_volume_relative_path_strips_root_for_mount_points() {
        let mut path = PathBuf::from(r"C:\Mounts\Data");
        push_volume_relative_path(&mut path, Path::new(r"\Windows\System32"));

        assert_eq!(path, PathBuf::from(r"C:\Mounts\Data\Windows\System32"));
    }
}
