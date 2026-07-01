//! Iterate the USN journal and print each entry with its resolved path when available.

use usn_journal_rs::{errors::UsnError, volume::Volume};

/// Run the example and print any top-level error.
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

/// Open a volume, stream the USN journal, and resolve each entry to a path.
fn run() -> Result<(), UsnError> {
    let drive_letter = 'D';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let journal = volume.journal();

    let path_resolver = volume.path_resolver();

    for result in journal.try_iter()? {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                match full_path {
                    Some(p) => println!("{entry} -> {}", p.display()),
                    None => println!("{entry}"),
                }
            }
            Err(e) => {
                eprintln!("Error reading USN entry: {e}");
                // Continue processing other entries
                continue;
            }
        }
    }

    Ok(())
}
