//! Demonstrate a custom multi-line formatter built on top of the public `UsnEntry` fields.

use std::path::Path;

use usn_journal_rs::Filetime;
use usn_journal_rs::journal::{UsnEntry, UsnJournal};
use usn_journal_rs::path::PathResolver;
use usn_journal_rs::volume::Volume;

use windows::Win32::{
    Foundation::{FILETIME, SYSTEMTIME},
    System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
};

/// Render a `UsnEntry` plus an optional resolved path as a multi-line string.
fn pretty_format<P: AsRef<Path>>(entry: &UsnEntry, full_path_opt: Option<P>) -> String {
    let mut output = String::new();
    output.push_str(&format!("{:<20}: {}\n", "USN", entry.usn));
    output.push_str(&format!(
        "{:<20}: {}\n",
        "Type",
        if entry.is_dir() { "Directory" } else { "File" }
    ));
    output.push_str(&format!("{:<20}: {}\n", "File ID", entry.fid));
    output.push_str(&format!("{:<20}: {}\n", "Parent File ID", entry.parent_fid));

    // Convert FILETIME to local time for display. On values that cannot be
    // converted, fall back to the raw FILETIME value.
    let timestamp_str = match format_local_filetime(entry.time) {
        Some(ts) => ts,
        None => format!("{:?}", entry.time),
    };
    output.push_str(&format!("{:<20}: {}\n", "Timestamp", timestamp_str));
    output.push_str(&format!(
        "{:<20}: {}\n",
        "Reason",
        entry.get_reason_string()
    ));

    if let Some(full_path) = full_path_opt {
        output.push_str(&format!(
            "{:<20}: {}\n",
            "Path",
            full_path.as_ref().to_string_lossy()
        ));
    } else {
        output.push_str(&format!(
            "{:<20}: {}\n",
            "Path",
            entry.file_name.to_string_lossy()
        ));
    }
    output
}

/// Formats a `FILETIME` as a local timestamp string.
fn format_local_filetime(filetime: Filetime) -> Option<String> {
    let utc_filetime: FILETIME = filetime.into();

    let mut utc_system_time = SYSTEMTIME::default();
    let mut local_system_time = SYSTEMTIME::default();

    unsafe {
        FileTimeToSystemTime(&utc_filetime, &mut utc_system_time).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc_system_time, &mut local_system_time).ok()?;
    }

    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local_system_time.wYear,
        local_system_time.wMonth,
        local_system_time.wDay,
        local_system_time.wHour,
        local_system_time.wMinute,
        local_system_time.wSecond,
    ))
}

/// Run the example and print a few journal entries in a multi-line format.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let volume = Volume::from_drive_letter('C')?;
    let usn_journal = UsnJournal::new(&volume);
    let mut path_resolver = PathResolver::new(&volume);

    for result in usn_journal.try_iter()?.take(10) {
        match result {
            Ok(entry) => {
                let full_path = path_resolver.resolve_path(&entry);
                println!("{}", pretty_format(&entry, full_path));
                println!("{}", "-".repeat(60));
            }
            Err(e) => eprintln!("Error reading USN entry: {e}"),
        }
    }

    Ok(())
}
