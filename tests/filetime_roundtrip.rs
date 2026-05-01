//! Integration tests for `Filetime` conversions.
//!
//! NOTE: `usn_journal_rs::time` is a private module so `Filetime` cannot be
//! named by external test code. These tests access it via type inference from
//! the public `RawMftEntry::si_created` field and manipulate the `pub u64`
//! inner field (`.0`) to cover the full value range.
//!
//! All tests are admin-gated: they print "skipping" and return when the OS
//! denies access to the volume.

use usn_journal_rs::{
    errors::UsnError,
    raw_mft::{RawMft, RawMftEntry},
    volume::Volume,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// 100-nanosecond intervals from the Windows epoch (1601-01-01) to the Unix
/// epoch (1970-01-01).  Mirrors the private `WINDOWS_TO_UNIX_OFFSET_100NS`
/// constant inside `usn_journal_rs::time`.
const WIN_EPOCH_OFFSET: u64 = 116_444_736_000_000_000;

/// Return the first `RawMftEntry` that has a non-zero `si_created` timestamp,
/// or `None` if the volume isn't accessible (e.g. non-elevated).
fn get_seed_entry() -> Option<usn_journal_rs::raw_mft::RawMftEntry> {
    let volume = Volume::from_drive_letter('C').ok()?;
    let raw_mft = RawMft::new(&volume).ok()?;
    raw_mft
        .try_iter()
        .ok()?
        .filter_map(|r: Result<RawMftEntry, UsnError>| r.ok())
        .take(2_000)
        .find(|e| e.si_created.as_u64() != 0)
}

// ─── as_u64 round-trip ────────────────────────────────────────────────────

#[test]
fn test_as_u64_roundtrip_many_values() {
    let seed = match get_seed_entry() {
        Some(e) => e,
        None => {
            eprintln!("filetime_roundtrip: skipping (requires admin or no usable entries found)");
            return;
        }
    };

    // Use the seed's si_created as the type-inferred Filetime.
    let mut ft = seed.si_created;
    let test_values: &[u64] = &[
        0,
        1,
        42,
        999_999,
        WIN_EPOCH_OFFSET,               // Unix epoch
        WIN_EPOCH_OFFSET + 10_000_000,  // 1 second after Unix epoch
        WIN_EPOCH_OFFSET + 1,           // 100 ns after Unix epoch
        u64::MAX / 2,
        u64::MAX - 1,
        u64::MAX,
    ];

    for &x in test_values {
        ft.0 = x;
        assert_eq!(ft.as_u64(), x, "as_u64 round-trip failed for {x}");
    }
}

// ─── try_to_system_time ───────────────────────────────────────────────────

#[test]
fn test_try_to_system_time_known_values() {
    let seed = match get_seed_entry() {
        Some(e) => e,
        None => {
            eprintln!("filetime_roundtrip: skipping (requires admin)");
            return;
        }
    };

    let mut ft = seed.si_created;

    // Windows epoch (0) should be representable as a pre-Unix SystemTime on Windows.
    ft.0 = 0;
    assert!(
        ft.try_to_system_time().is_some(),
        "Windows epoch (0) should convert to Some(SystemTime)"
    );

    // Unix epoch must map to exactly UNIX_EPOCH.
    ft.0 = WIN_EPOCH_OFFSET;
    assert_eq!(
        ft.try_to_system_time(),
        Some(UNIX_EPOCH),
        "Filetime(WIN_EPOCH_OFFSET) should equal UNIX_EPOCH"
    );

    // u64::MAX must not panic (may legitimately return None on overflow).
    ft.0 = u64::MAX;
    let _ = ft.try_to_system_time();
}

#[test]
fn test_try_to_system_time_current_roundtrip() {
    let seed = match get_seed_entry() {
        Some(e) => e,
        None => {
            eprintln!("filetime_roundtrip: skipping (requires admin)");
            return;
        }
    };

    let now = SystemTime::now();
    let dur = now.duration_since(UNIX_EPOCH).unwrap();
    // Convert "now" to a FILETIME u64 value (100-ns intervals since 1601-01-01).
    let now_filetime_val = WIN_EPOCH_OFFSET + (dur.as_nanos() / 100) as u64;

    let mut ft = seed.si_created;
    ft.0 = now_filetime_val;

    let converted = ft
        .try_to_system_time()
        .expect("current SystemTime must be representable");

    // Round-trip tolerance: 200 ns (2 ticks of 100-ns resolution).
    let diff_ns = now
        .duration_since(converted)
        .or_else(|e| Ok::<_, ()>(e.duration()))
        .unwrap()
        .as_nanos();
    assert!(
        diff_ns < 200,
        "try_to_system_time round-trip should be within 200 ns, got {diff_ns} ns"
    );
}

// ─── to_unix_seconds ─────────────────────────────────────────────────────

#[test]
fn test_to_unix_seconds_known_values() {
    let seed = match get_seed_entry() {
        Some(e) => e,
        None => {
            eprintln!("filetime_roundtrip: skipping (requires admin)");
            return;
        }
    };

    let mut ft = seed.si_created;

    // Exactly at Unix epoch: should be 0.
    ft.0 = WIN_EPOCH_OFFSET;
    assert_eq!(ft.to_unix_seconds(), 0, "Unix epoch should be 0 unix-seconds");

    // 1 second after: should be 1.
    ft.0 = WIN_EPOCH_OFFSET + 10_000_000;
    assert_eq!(ft.to_unix_seconds(), 1, "Unix epoch + 1s should be 1");

    // 1 second before: should be -1.
    ft.0 = WIN_EPOCH_OFFSET - 10_000_000;
    assert_eq!(ft.to_unix_seconds(), -1, "Unix epoch − 1s should be -1");
}

#[test]
fn test_to_unix_seconds_matches_system_time_now() {
    let seed = match get_seed_entry() {
        Some(e) => e,
        None => {
            eprintln!("filetime_roundtrip: skipping (requires admin)");
            return;
        }
    };

    let now = SystemTime::now();
    let dur = now.duration_since(UNIX_EPOCH).unwrap();
    let now_filetime_val = WIN_EPOCH_OFFSET + (dur.as_nanos() / 100) as u64;

    let mut ft = seed.si_created;
    ft.0 = now_filetime_val;

    let expected_secs = dur.as_secs() as i64;
    let got_secs = ft.to_unix_seconds();
    assert!(
        (got_secs - expected_secs).abs() <= 2,
        "to_unix_seconds should be within ±2 s of SystemTime::now(); expected ~{expected_secs}, got {got_secs}"
    );
}

// ─── to_unix_nanos ────────────────────────────────────────────────────────

#[test]
fn test_to_unix_nanos_known_values() {
    let seed = match get_seed_entry() {
        Some(e) => e,
        None => {
            eprintln!("filetime_roundtrip: skipping (requires admin)");
            return;
        }
    };

    let mut ft = seed.si_created;

    // Exactly at Unix epoch: should be 0.
    ft.0 = WIN_EPOCH_OFFSET;
    assert_eq!(ft.to_unix_nanos(), 0);

    // One 100-ns tick after Unix epoch: should be 100.
    ft.0 = WIN_EPOCH_OFFSET + 1;
    assert_eq!(ft.to_unix_nanos(), 100);

    // One second (10_000_000 ticks) after Unix epoch: should be 1_000_000_000.
    ft.0 = WIN_EPOCH_OFFSET + 10_000_000;
    assert_eq!(ft.to_unix_nanos(), 1_000_000_000);
}
