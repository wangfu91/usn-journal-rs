use usn_journal_rs::{path_resolver::UsnJournalPathResolver, usn_journal::UsnJournal};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let journal = UsnJournal::new_from_drive_letter(drive_letter)?;

    let mut path_resolver = UsnJournalPathResolver::new(&journal);

    for entry in journal.iter() {
        let full_path = path_resolver.resolve_path(&entry);
        println!(
            "usn={:?}, file_id={:?}, path={:?}",
            entry.usn, entry.fid, full_path
        );
    }

    Ok(())
}
