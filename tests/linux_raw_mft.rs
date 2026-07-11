#![cfg(target_os = "linux")]

use usn_journal_rs::{raw_mft::RawMft, volume::Volume};

#[test]
fn mounted_ntfs_raw_mft_smoke_test() {
    let Some(source) = std::env::var_os("USN_TEST_VOLUME") else {
        eprintln!("linux_raw_mft: skipping; set USN_TEST_VOLUME to an NTFS mount or device");
        return;
    };
    let source = std::path::Path::new(&source);
    let open_result = if source.starts_with("/dev") {
        Volume::from_device_path(source)
    } else {
        Volume::from_mount_point(source)
    };
    let volume = match open_result {
        Ok(volume) => volume,
        Err(error) => {
            eprintln!("linux_raw_mft: skipping unavailable read-only source: {error}");
            return;
        }
    };
    let raw_mft = RawMft::new(&volume).expect("parse NTFS boot sector and discover $MFT");
    let entries: Vec<_> = raw_mft
        .try_iter()
        .expect("create raw $MFT iterator")
        .take(128)
        .collect::<Result<_, _>>()
        .expect("parse raw $MFT prefix");
    assert!(
        !entries.is_empty(),
        "raw $MFT prefix should contain records"
    );

    let first = &entries[0];
    let reread = raw_mft
        .read_record(first.record_number)
        .expect("reread first record")
        .expect("first record should remain addressable");
    assert_eq!(reread.file_reference, first.file_reference);
}
