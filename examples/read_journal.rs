use usn_journal_rs::{
    errors::UsnError,
    journal::UsnJournal,
    path::{JournalPathResolver, PathResolveTrait},
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
    let usn_journal = UsnJournal::new(volume);

    let mut path_resolver = JournalPathResolver::new_with_cache(&usn_journal);

    for entry in usn_journal.iter()? {
        let full_path = path_resolver.resolve_path(&entry);
        println!(
            "usn={}, file_id={}, path={:?}",
            entry.usn, entry.fid, full_path
        );
    }

    Ok(())
}
