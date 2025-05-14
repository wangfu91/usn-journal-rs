use usn_journal_rs::{errors::UsnError, mft::Mft, path::MftPathResolver, volume::Volume};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = 'C';
    let volume = Volume::from_drive_letter(drive_letter)?;
    let mft = Mft::new(volume)?;

    let mut path_resolver = MftPathResolver::new(&mft);

    for entry in mft.iter() {
        let full_path = path_resolver.resolve_path(&entry);
        println!("fid={}, path={:?}", entry.fid, full_path);
    }

    Ok(())
}
