use usn_journal_rs::{mft::Mft, path_resolver::MftPathResolver};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let mft = Mft::new_from_drive_letter(drive_letter)?;

    let mut path_resolver = MftPathResolver::new(&mft);

    for entry in mft.iter() {
        let full_path = path_resolver.resolve_path(&entry);
        println!("fid={:?}, path={:?}", entry.fid, full_path);
    }

    Ok(())
}
