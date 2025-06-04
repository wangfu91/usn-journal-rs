use usn_journal_rs::{errors::UsnError, mft::Mft, path::PathResolver, volume::Volume};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = 'C';
    let volume = Volume::from_drive_letter(drive_letter)?;

    let mft = Mft::new(&volume);

    let mut path_resolver = PathResolver::new_with_cache(&volume);

    for entry in mft.iter() {
        let full_path = path_resolver.resolve_path(&entry);
        println!("{}", entry.pretty_format(full_path));
    }

    Ok(())
}
