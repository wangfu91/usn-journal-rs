use std::{env, error::Error, time::Duration};

use usn_journal_rs::{raw_mft::RawMft, volume::Volume};

fn main() -> Result<(), Box<dyn Error>> {
    let drive = env::args()
        .nth(1)
        .and_then(|arg| arg.chars().next())
        .map(|ch| ch.to_ascii_uppercase())
        .unwrap_or('C');

    let volume = Volume::from_drive_letter(drive)?;
    let mft = RawMft::new(&volume)?;
    let profile = mft.profile()?;

    println!("raw_mft profile");
    println!("  drive:                         {drive}:");
    println!("  start_record:                  {}", profile.start_record);
    println!("  end_record:                    {}", profile.end_record);
    println!("  buffer_bytes:                  {}", profile.buffer_bytes);
    println!(
        "  records_examined:              {}",
        profile.records_examined
    );
    println!(
        "  records_skipped_unused:        {}",
        profile.records_skipped_unused
    );
    println!("  sparse_holes:                  {}", profile.sparse_holes);
    println!("  invalid_records:               {}", profile.invalid_records);
    println!(
        "  extension_records_skipped:     {}",
        profile.extension_records_skipped
    );
    println!("  parse_errors:                  {}", profile.parse_errors);
    println!("  records_yielded:               {}", profile.records_yielded);
    println!(
        "  attr_list_enrichments:         {}",
        profile.attr_list_enrichments_attempted
    );
    println!(
        "  enrichments_with_loads:        {}",
        profile.attr_list_enrichments_with_extension_loads
    );
    println!(
        "  attr_list_ext_records_ref:     {}",
        profile.attr_list_extension_records_referenced
    );
    println!(
        "  attr_list_ext_records_loaded:  {}",
        profile.attr_list_extension_records_loaded
    );
    println!(
        "  total_elapsed:                 {}",
        format_duration(profile.total_elapsed)
    );
    println!(
        "  bitmap_check_elapsed:          {}",
        format_duration(profile.bitmap_check_elapsed)
    );
    println!(
        "  record_offset_elapsed:         {}",
        format_duration(profile.record_offset_elapsed)
    );
    println!(
        "  borrow_elapsed:                {}",
        format_duration(profile.borrow_elapsed)
    );
    println!(
        "  validate_elapsed:              {}",
        format_duration(profile.validate_elapsed)
    );
    println!(
        "  parse_elapsed:                 {}",
        format_duration(profile.parse_elapsed)
    );
    println!(
        "  entry_build_elapsed:           {}",
        format_duration(profile.entry_build_elapsed)
    );
    println!(
        "  attr_list_enrich_elapsed:      {}",
        format_duration(profile.attr_list_enrich_elapsed)
    );
    if profile.total_elapsed.as_secs_f64() > 0.0 {
        println!(
            "  yielded_records_per_second:    {:.0}",
            profile.records_yielded as f64 / profile.total_elapsed.as_secs_f64()
        );
    }

    Ok(())
}

fn format_duration(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}
