mod common;

use usn_journal_rs::{errors::UsnError, volume::Volume};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = common::drive_letter_from_args_or('C');
    let volume = Volume::from_drive_letter(drive_letter)?;
    let mft = volume.mft();
    let mut path_resolver = volume.path_resolver_with_cache();

    for result in mft.iter() {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                println!("{}", entry.pretty_format(full_path));
            }
            Err(e) => {
                eprintln!("Error reading MFT entry: {e}");
                continue;
            }
        }
    }

    Ok(())
}
