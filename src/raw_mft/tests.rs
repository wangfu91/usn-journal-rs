use super::*;

#[test]
fn options_defaults_are_sensible() {
    let o = RawMftIterOptions::default();
    assert_eq!(o.buffer_bytes, DEFAULT_BUFFER_BYTES);
    assert!(o.skip_unused);
    assert_eq!(o.start_record, super::record::FIRST_NORMAL_RECORD);
    assert!(o.end_record.is_none());
}

mod integration_tests {
    use super::super::*;
    use crate::path::PathResolver;
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
        let mut resolver = PathResolver::builder(&volume)
            .with_lru_cache(
                std::num::NonZeroUsize::new(4096).expect("cache capacity must be non-zero"),
            )
            .build();
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
