use usn_journal_rs::{
    path_resolver::PathResolver,
    usn_journal::{self, UsnJournal},
    utils,
};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let volume_handle = utils::get_volume_handle(drive_letter)?;

    let journal_data = usn_journal::query(volume_handle, true)?;

    let enum_options = usn_journal::EnumOptions {
        start_usn: journal_data.NextUsn,
        only_on_close: true,
        wait_for_more: true,
        ..Default::default()
    };

    let mut journal =
        UsnJournal::new_with_options(volume_handle, journal_data.UsnJournalID, enum_options);

    let mut path_resolver = PathResolver::new(volume_handle, drive_letter);

    for entry in journal.iter() {
        let full_path = path_resolver.resolve_path_from_usn(&entry);
        println!(
            "usn={:?}, reason={:?}, path={:?}",
            entry.usn,
            entry.reason_to_string(),
            full_path
        );
    }

    Ok(())
}
