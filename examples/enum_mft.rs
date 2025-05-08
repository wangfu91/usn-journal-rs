use usn_journal_rs::{mft::Mft, path_resolver::PathResolver, utils};

fn main() -> anyhow::Result<()> {
    let drive_letter = 'C';

    let volume_handle = utils::get_volume_handle(drive_letter)?;

    let mft = Mft::new(volume_handle);

    let mut path_resolver = PathResolver::new(volume_handle, drive_letter);

    for entry in mft {
        let full_path = path_resolver.resolve_path(&entry);
        println!("fid={:?}, path={:?}", entry.fid, full_path);
    }

    Ok(())
}
