// Demonstrates a verbose, multi-line "pretty" formatter for `UsnEntry`,
// equivalent to the previously built-in `UsnEntry::pretty_format` method.
//
// The library now provides a compact one-line `Display` impl. This example
// shows how callers can build their own verbose formatter on top of the
// public fields when richer output is desired.

use std::path::Path;
use std::time::SystemTime;

use usn_journal_rs::journal::{UsnEntry, UsnJournal};
use usn_journal_rs::path::PathResolver;
use usn_journal_rs::volume::Volume;

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

    // Convert FILETIME to a SystemTime for display. On platforms or values that
    // cannot be represented, fall back to the raw FILETIME value.
    let timestamp_str = match entry.time.to_system_time() {
        Some(st) => format_system_time(st),
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

/// Formats a `SystemTime` as an ISO-8601-like UTC string.
fn format_system_time(st: SystemTime) -> String {
    match st.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs() as i64;
            format_unix_seconds_utc(secs)
        }
        Err(e) => {
            // Time is before UNIX epoch.
            let secs = -(e.duration().as_secs() as i64);
            format_unix_seconds_utc(secs)
        }
    }
}

fn format_unix_seconds_utc(secs: i64) -> String {
    // Days from epoch and seconds within day.
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let h = sod / 3600;
    let m = (sod % 3600) / 60;
    let s = sod % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02} UTC")
}

fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Convert days since 1970-01-01 to (year, month, day).
    // Algorithm by Howard Hinnant: http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

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
