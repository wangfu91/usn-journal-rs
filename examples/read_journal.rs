use usn_journal_rs::{errors::UsnError, journal::UsnJournal, path::PathResolver, volume::Volume};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = 'D';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let usn_journal = UsnJournal::new(&volume);

    let mut path_resolver = PathResolver::new_with_cache(&volume);

    for result in usn_journal.iter()? {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                println!("{}", entry.pretty_format(full_path));
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
