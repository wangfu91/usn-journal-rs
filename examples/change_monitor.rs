//! Follow the live USN journal from its current tail and print new records as they arrive.

mod common;

use usn_journal_rs::{errors::UsnError, journal::JournalIterOptions, volume::Volume};

/// Run the example and print any top-level error.
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

/// Query the current journal tail and then block waiting for new records.
fn run() -> Result<(), UsnError> {
    let drive_letter = common::drive_letter_from_args_or('C');
    let volume = Volume::from_drive_letter(drive_letter)?;
    let journal = volume.journal();

    let journal_data = journal.query_or_create()?;

    let enum_options = JournalIterOptions::builder()
        .start_usn(journal_data.next_usn)
        .only_on_close(false)
        .wait_for_more(true)
        .build()?;

    let path_resolver = volume.path_resolver();

    for result in journal.try_iter_with_options(enum_options)? {
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
