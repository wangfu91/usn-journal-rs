use usn_journal_rs::{
    errors::UsnError,
    journal::{self, UsnJournal},
    path::JournalPathResolver,
    volume::Volume,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = 'C';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let journal = UsnJournal::new(volume)?;

    let enum_options = journal::EnumOptions {
        start_usn: journal.next_usn,
        only_on_close: true,
        wait_for_more: true,
        ..Default::default()
    };

    let mut path_resolver = JournalPathResolver::new(&journal);

    for entry in journal.iter_with_options(enum_options) {
        let full_path = path_resolver.resolve_path(&entry);
        println!(
            "usn={}, reason={}, path={:?}",
            entry.usn,
            entry.reason_to_string(),
            full_path
        );
    }

    Ok(())
}
