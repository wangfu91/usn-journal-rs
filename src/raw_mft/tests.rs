use super::*;

#[test]
fn options_defaults_are_sensible() {
    let o = RawMftScanOptions::default();
    assert_eq!(o.buffers().main(), DEFAULT_BUFFER_BYTES);
    assert_eq!(o.buffers().attr(), DEFAULT_ATTR_BUFFER_BYTES);
    assert!(!o.include_unused_records());
    assert!(o.entry().collect_alternate_data_streams());
    assert!(o.entry().collect_data_run_summary());
    assert!(o.entry().collect_dos_file_name_links());
    assert_eq!(
        o.range().start_record(),
        super::layout::record::FIRST_NORMAL_RECORD
    );
    assert!(o.range().end_record().is_none());
}

#[test]
fn raw_mft_entry_display_is_compact() {
    use crate::{Fid, FileAttributes, Filetime};

    let entry = RawMftEntry {
        record_number: 42,
        sequence_number: 1,
        file_reference: Fid::from_parts(42, 1),
        parent_reference: Fid::from_parts(5, 1),
        base_record_reference: 0,
        hard_link_count: 2,
        flags: 0,
        is_used: true,
        is_directory: false,
        is_reparse_point: false,
        reparse_tag: None,
        namespace: FileNameNamespace::Win32,
        file_name: std::ffi::OsString::from("file.txt"),
        si_created: Filetime::new(0),
        si_modified: Filetime::new(0),
        si_mft_modified: Filetime::new(0),
        si_accessed: Filetime::new(0),
        si_file_attributes: FileAttributes::empty(),
        fn_created: Filetime::new(0),
        fn_modified: Filetime::new(0),
        fn_mft_modified: Filetime::new(0),
        fn_accessed: Filetime::new(0),
        real_size: 1234,
        allocated_size: 4096,
        has_unnamed_data: true,
        is_resident: false,
        is_sparse: false,
        is_compressed: false,
        is_encrypted: false,
        data_run_summary: None,
        alternate_data_streams: Box::default(),
        links: Box::default(),
    };

    let text = entry.to_string();
    assert!(text.contains("raw #42"));
    assert!(text.contains("FILE"));
    assert!(text.contains("links=2"));
    assert!(text.contains("size=1234"));
    assert!(text.contains("\"file.txt\""));
}

mod integration_tests {
    use super::super::*;
    use crate::volume::Volume;
    use std::env;

    fn pick_drive() -> char {
        env::var("USN_TEST_DRIVE")
            .ok()
            .and_then(|s| s.chars().next())
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('C')
    }

    fn open_volume_or_skip() -> Option<Volume> {
        match Volume::from_drive_letter(pick_drive()) {
            Ok(v) => Some(v),
            Err(UsnError::NotElevated) => {
                eprintln!("skipping: requires admin privileges");
                None
            }
            Err(e) => {
                eprintln!("skipping: {e}");
                None
            }
        }
    }

    #[test]
    fn raw_mft_path_resolver_roundtrip() {
        let Some(volume) = open_volume_or_skip() else {
            return;
        };
        let mft = match RawMft::new(&volume) {
            Ok(m) => m,
            Err(UsnError::UnsupportedFilesystem(_)) => return,
            Err(e) => panic!("RawMft::new failed: {e}"),
        };
        let resolver = mft.path_resolver().expect("raw mft path resolver");
        let mut resolved_any = false;
        // Cap the search so the test stays bounded on huge volumes.
        for r in mft.try_iter().expect("iter").flatten().take(20_000) {
            if r.is_directory || r.file_name.is_empty() {
                continue;
            }
            if let Some(path) = resolver.resolve_path(&r) {
                let s = path.to_string_lossy();
                if s.len() > 3 {
                    resolved_any = true;
                    break;
                }
            }
        }
        assert!(
            resolved_any,
            "expected at least one resolvable path on the test drive"
        );
    }

    #[test]
    fn raw_mft_refs_returns_unsupported() {
        // D: is ReFS on the developer machine; skip unless USN_TEST_DRIVE
        // explicitly points at a non-NTFS drive or D: exists.
        let drive = env::var("USN_REFS_TEST_DRIVE")
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('D')
            .to_ascii_uppercase();
        let volume = match Volume::from_drive_letter(drive) {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: ReFS drive {drive} not available");
                return;
            }
        };
        match RawMft::new(&volume) {
            Err(UsnError::UnsupportedFilesystem(_)) => {}
            Err(other) => eprintln!("non-NTFS produced: {other}"),
            Ok(_) => {
                eprintln!("note: drive {drive} is NTFS; UnsupportedFilesystem not exercised")
            }
        }
    }
}
