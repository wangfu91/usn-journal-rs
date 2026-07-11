use std::{env, error::Error, path::Path};

use usn_journal_rs::{raw_mft::RawMft, volume::Volume};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let source = args.next();
    let limit = match args.next().as_deref() {
        None | Some("all") => usize::MAX,
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| format!("invalid record limit {value:?}; pass an integer or 'all'"))?,
    };
    #[cfg(windows)]
    let volume = {
        let drive = source.and_then(|value| value.chars().next()).unwrap_or('C');
        Volume::from_drive_letter(drive)?
    };
    #[cfg(target_os = "linux")]
    let volume = {
        let source = source
            .ok_or("usage: raw_mft_read <NTFS mount point or device path> [record limit|all]")?;
        if source.starts_with("/dev/") {
            Volume::from_device_path(source)?
        } else {
            Volume::from_mount_point(Path::new(&source))?
        }
    };

    let mft = RawMft::new(&volume)?;
    let resolver = mft.path_resolver()?;
    println!(
        "Reading allocated NTFS records in MFT record order (starting at record 24; paths are snapshot-derived)"
    );
    println!(
        "{:<10} {:<18} {:<5} PATH",
        "RECORD", "FILE_REFERENCE", "TYPE"
    );
    let mut emitted = 0usize;
    for entry in mft.try_iter()?.take(limit) {
        let entry = entry?;
        let path = resolver
            .resolve_path(&entry)
            .unwrap_or_else(|| entry.file_name.clone().into());
        println!(
            "{:<10} {:016x} {:<5} {}",
            entry.record_number,
            entry.file_reference,
            if entry.is_directory { "DIR" } else { "FILE" },
            path.display(),
        );
        emitted += 1;
    }
    eprintln!("Emitted {emitted} allocated MFT records");
    Ok(())
}
