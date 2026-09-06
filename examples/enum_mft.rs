//! Enumerate `FSCTL_ENUM_USN_DATA` records and print each entry with its resolved path.

mod common;

use usn_journal_rs::{errors::UsnError, volume::Volume};

/// Run the example and print any top-level error.
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

/// Open a volume, enumerate MFT entries, and resolve each one to a path.
fn run() -> Result<(), UsnError> {
    let drive_letter = common::drive_letter_from_args_or('C');
    let volume = Volume::from_drive_letter(drive_letter)?;
    let mft = volume.mft();
    let path_resolver = volume.path_resolver();

    for result in mft.try_iter()? {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                match full_path {
                    Some(p) => println!("{entry} -> {}", p.display()),
                    None => println!("{entry}"),
                }
            }
            Err(e) => {
                eprintln!("Error reading MFT entry: {e}");
                continue;
            }
        }
    }

    Ok(())
}
