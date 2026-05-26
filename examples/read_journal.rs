use usn_journal_rs::{errors::UsnError, volume::Volume};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = 'D';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let usn_journal = volume.journal();

    let mut path_resolver = volume.path_resolver_with_cache();

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
