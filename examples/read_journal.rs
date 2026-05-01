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

    let mut path_resolver = PathResolver::new(&volume).with_lru_cache(std::num::NonZeroUsize::new(4096).unwrap());

    for result in usn_journal.try_iter()? {
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
