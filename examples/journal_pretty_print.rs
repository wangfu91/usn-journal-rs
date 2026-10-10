//! Print detailed journal records with optional resolved paths.

use usn_journal_rs::{journal::UsnJournal, path::PathResolver, volume::Volume};

/// Run the example and print a few journal entries in a multi-line format.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let volume = Volume::from_drive_letter('C')?;
    let usn_journal = UsnJournal::new(&volume);
    let path_resolver = PathResolver::new(&volume);

    for result in usn_journal.try_iter()?.take(10) {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                println!("{}", entry.pretty_format(full_path));
                println!("{}", "-".repeat(60));
            }
            Err(e) => eprintln!("Error reading USN entry: {e}"),
        }
    }

    Ok(())
}
