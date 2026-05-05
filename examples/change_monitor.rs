//! Follow the live USN journal from its current tail and print new records as they arrive.

use usn_journal_rs::{
    errors::UsnError,
    journal::{JournalIterOptions, UsnJournal},
    path::PathResolver,
    volume::Volume,
};

/// Run the example and print any top-level error.
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

/// Query the current journal tail and then block waiting for new records.
fn run() -> Result<(), UsnError> {
    let drive_letter = 'D';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let usn_journal = UsnJournal::new(&volume);

    let journal_data = usn_journal.query(true)?;

    let enum_options = JournalIterOptions::builder()
        .start_usn(journal_data.next_usn)
        .only_on_close(false)
        .wait_for_more(true)
        .build();

    let mut path_resolver = PathResolver::new(&volume);

    for result in usn_journal.try_iter_with_options(enum_options)? {
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
                continue;
            }
        }
    }

    Ok(())
}
