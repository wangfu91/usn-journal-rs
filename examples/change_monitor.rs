use usn_journal_rs::{
    path_resolver::UsnJournalPathResolver,
    usn_journal::{self, UsnJournal},
};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let journal = UsnJournal::new_from_drive_letter(drive_letter)?;

    let enum_options = usn_journal::EnumOptions {
        start_usn: journal.next_usn,
        only_on_close: true,
        wait_for_more: true,
        ..Default::default()
    };

    let mut path_resolver = UsnJournalPathResolver::new(&journal);

    for entry in journal.iter_with_options(enum_options) {
        let full_path = path_resolver.resolve_path(&entry);
        println!(
            "usn={:?}, reason={:?}, path={:?}",
            entry.usn,
            entry.reason_to_string(),
            full_path
        );
    }

    Ok(())
}
