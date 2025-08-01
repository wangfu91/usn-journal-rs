use usn_journal_rs::{
    errors::UsnError,
    journal::{self, UsnJournal},
    path::PathResolver,
    volume::Volume,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = 'D';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let usn_journal = UsnJournal::new(&volume);

    let journal_data = usn_journal.query(true)?;

    let enum_options = journal::EnumOptions {
        start_usn: journal_data.next_usn,
        only_on_close: false,
        wait_for_more: true,
        ..Default::default()
    };

    let mut path_resolver = PathResolver::new(&volume);

    for result in usn_journal.iter_with_options(enum_options)? {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                println!("{}", entry.pretty_format(full_path));
            }
            Err(e) => {
                eprintln!("Error reading USN entry: {e}");
                continue;
            }
        }
    }

    Ok(())
}
