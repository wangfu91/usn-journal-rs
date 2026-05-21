//! Walk every record in the raw `$MFT` of a drive and print a one-line
//! summary that highlights the rich metadata `RawMft` exposes (full
//! timestamps, real / allocated size, alternate data streams, sparse and
//! compressed flags).
//!
//! Run with administrator privileges:
//!
//! ```text
//! cargo run --example raw_mft_serial_read -- C
//! ```

use std::env;
use usn_journal_rs::{
    errors::UsnError,
    raw_mft::{RawMft, RawMftScanOptions},
    volume::Volume,
};

const MAX_ENTRIES: usize = 1_000;

/// Run the example and print any top-level error.
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Open the raw `$MFT`, iterate records, and print a compact metadata summary.
fn run() -> Result<(), UsnError> {
    let drive_letter = env::args()
        .nth(1)
        .and_then(|s| s.chars().next())
        .unwrap_or('C')
        .to_ascii_uppercase();

    let volume = Volume::from_drive_letter(drive_letter)?;
    let mft = RawMft::new(&volume)?;
    let resolver = mft.path_resolver()?;

    println!(
        "$MFT: {} records, cluster_size={}, file_record_size={}",
        mft.record_count(),
        mft.cluster_size(),
        mft.file_record_size()
    );

    let mut count = 0u64;
    let options = RawMftScanOptions::builder()
        .include_unused_records(true)
        .collect_alternate_data_streams(true)
        .collect_data_run_summary(true)
        .collect_dos_file_name_links(true)
        .build();
    for result in mft.try_iter_with_options(options)?.take(MAX_ENTRIES) {
        match result {
            Ok(entry) => {
                let path = resolver.resolve_path(&entry);
                let path_display = path
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
                    path_display,
                );
                count += 1;
            }
            Err(e) => eprintln!("error: {e}"),
        }
    }
    eprintln!("Done. Yielded {count} records.");
    Ok(())
}
