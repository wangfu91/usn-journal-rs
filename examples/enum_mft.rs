use usn_journal_rs::{mft::Mft, path_resolver::PathResolver, utils};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let volume_handle = utils::get_volume_handle(drive_letter)?;

    let mut mft = Mft::new(volume_handle);

    let mut path_resolver = PathResolver::new(volume_handle, drive_letter);

    for entry in mft.iter() {
        let full_path = path_resolver.resolve_path_from_mft(&entry);
        println!("fid={:?}, path={:?}", entry.fid, full_path);
    }

    Ok(())
}
