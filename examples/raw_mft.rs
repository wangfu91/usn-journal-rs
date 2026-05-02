//! Walk every record in the raw `$MFT` of a drive and print a one-line
//! summary that highlights the rich metadata `RawMft` exposes (full
//! timestamps, real / allocated size, alternate data streams, sparse and
//! compressed flags).
//!
//! Run with administrator privileges:
//!
//! ```text
//! cargo run --example raw_mft -- C
//! ```

use std::env;
use std::num::NonZeroUsize;

use usn_journal_rs::{errors::UsnError, path::PathResolver, raw_mft::RawMft, volume::Volume};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), UsnError> {
    let drive_letter = env::args()
        .nth(1)
        .and_then(|s| s.chars().next())
        .unwrap_or('C')
        .to_ascii_uppercase();

    let volume = Volume::from_drive_letter(drive_letter)?;
    let mft = RawMft::new(&volume)?;
    let mut resolver = PathResolver::builder(&volume)
        .with_lru_cache(NonZeroUsize::new(4096).expect("cache capacity must be non-zero"))
        .build();

    println!(
        "$MFT: {} records, cluster_size={}, file_record_size={}",
        mft.record_count(),
        mft.cluster_size(),
        mft.file_record_size()
    );

    let mut count = 0u64;
    for result in mft.try_iter()? {
        match result {
            Ok(entry) => {
                let path = resolver.resolve_path(&entry);
                let path_disp = path
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| entry.file_name.to_string_lossy().to_string());
                let kind = if entry.is_directory { "DIR " } else { "FILE" };
                println!(
                    "{kind} #{:>10} size={:>12} alloc={:>12} links={} ads={} sparse={} compressed={} {}",
                    entry.record_number,
                    entry.real_size,
                    entry.allocated_size,
                    entry.hard_link_count,
                    entry.alternate_data_streams.len(),
                    entry.is_sparse,
                    entry.is_compressed,
                    path_disp,
                );
                count += 1;
            }
            Err(e) => eprintln!("error: {e}"),
        }
    }
    eprintln!("Done. Yielded {count} records.");
    Ok(())
}
