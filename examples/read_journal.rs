use usn_journal_rs::{
    path_resolver::PathResolver,
    usn_journal::{self, UsnJournal},
    utils,
};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let volume_handle = utils::get_volume_handle(drive_letter)?;

    let journal_data = usn_journal::query(volume_handle, true)?;

    let journal = UsnJournal::new(volume_handle, journal_data.UsnJournalID);

    let mut path_resolver = PathResolver::new(volume_handle, drive_letter);

    for entry in journal {
        let full_path = path_resolver.resolve_path_from_usn(&entry);
        println!(
            "usn={:?}, file_id={:?}, path={:?}",
            entry.usn, entry.fid, full_path
        );
    }

    Ok(())
}
