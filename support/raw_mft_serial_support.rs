#![allow(dead_code)]

//! Shared helpers for the serial raw-`$MFT` read benchmark and its
//! exact-match profiling example.
//!
//! The serial read path produces a full [`RawMftEntry`] per visible record.
//! Both the Criterion bench and the profiling executable drive the exact same
//! [`run_serial_read`] workload so flamegraph and ETW results stay aligned with
//! the benchmarked code.

use std::{env, num::NonZeroUsize};

use usn_journal_rs::{
    errors::UsnError,
    raw_mft::{RawMft, RawMftScanOptions},
    volume::Volume,
};

/// First normal FILE record number in the NTFS `$MFT`.
const FIRST_NORMAL_RECORD: u64 = 24;

/// Environment-driven configuration for the serial read benchmark.
#[derive(Debug, Clone)]
pub struct SerialConfig {
    /// Drive letter used for the raw volume.
    pub drive: char,
    /// First logical record number to include.
    pub start_record: u64,
    /// Optional exclusive end record number; `None` means the full `$MFT`.
    pub end_record: Option<u64>,
    /// Optional cap on the number of visible records consumed per run.
    pub max_records: Option<usize>,
    /// Whether to collect named alternate data streams.
    pub collect_alternate_data_streams: bool,
    /// Whether to summarize non-resident data runs.
    pub collect_data_run_summary: bool,
    /// Whether to retain shadowed DOS 8.3 `$FILE_NAME` links.
    pub collect_dos_file_name_links: bool,
    /// Whether to include records the `$MFT` `$BITMAP` marks unused.
    pub include_unused_records: bool,
}

impl SerialConfig {
    /// Build a configuration from environment variables and defaults.
    pub fn from_env() -> Self {
        Self {
            drive: pick_drive(),
            start_record: parse_env_u64("USN_RAW_MFT_SERIAL_START_RECORD")
                .unwrap_or(FIRST_NORMAL_RECORD),
            end_record: parse_env_u64("USN_RAW_MFT_SERIAL_END_RECORD"),
            max_records: parse_env_usize("USN_RAW_MFT_SERIAL_MAX_RECORDS"),
            collect_alternate_data_streams: parse_env_bool("USN_RAW_MFT_SERIAL_COLLECT_ADS", true),
            collect_data_run_summary: parse_env_bool("USN_RAW_MFT_SERIAL_COLLECT_RUNS", true),
            collect_dos_file_name_links: parse_env_bool("USN_RAW_MFT_SERIAL_COLLECT_DOS", true),
            include_unused_records: parse_env_bool("USN_RAW_MFT_SERIAL_INCLUDE_UNUSED", false),
        }
    }

    /// Build the scan options that drive [`RawMft::try_iter_with_options`].
    pub fn scan_options(&self) -> RawMftScanOptions {
        RawMftScanOptions::builder()
            .include_unused_records(self.include_unused_records)
            .collect_alternate_data_streams(self.collect_alternate_data_streams)
            .collect_data_run_summary(self.collect_data_run_summary)
            .collect_dos_file_name_links(self.collect_dos_file_name_links)
            .start_record(self.start_record)
            .end_record(self.end_record)
            .build()
    }
}

/// Compact, work-touching summary returned by the serial read workload.
///
/// Every field is derived from a materialized [`RawMftEntry`] so the optimizer
/// cannot elide name/link/ADS materialization.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SerialSummary {
    /// Number of visible base records yielded.
    pub records: u64,
    /// Number of records that failed to parse mid-scan.
    pub errors: u64,
    /// Sum of all file-name lengths (in UTF-16-ish chars) materialized.
    pub name_chars: u64,
    /// Sum of all retained hard-link names.
    pub links: u64,
    /// Sum of all alternate-data-stream entries.
    pub ads: u64,
    /// Running XOR of logical sizes, used as a cheap anti-elision checksum.
    pub size_checksum: u64,
}

/// Read the drive letter to benchmark from `USN_TEST_DRIVE`.
pub fn pick_drive() -> char {
    env::var("USN_TEST_DRIVE")
        .ok()
        .and_then(|value| value.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

/// Open the requested drive, or report why the bench should be skipped.
pub fn open_volume(drive: char) -> Option<Volume> {
    match Volume::from_drive_letter(drive) {
        Ok(volume) => Some(volume),
        Err(UsnError::NotElevated) => {
            eprintln!("skipping bench: requires admin privileges");
            None
        }
        Err(error) => {
            eprintln!("skipping bench: {error}");
            None
        }
    }
}

/// Print the serial read configuration to stderr.
pub fn print_serial_config(config: &SerialConfig) {
    eprintln!(
        "raw_mft serial read config: drive={} start_record={} end_record={} max_records={} ads={} runs={} dos={} unused={}",
        config.drive,
        config.start_record,
        config
            .end_record
            .map(|value| value.to_string())
            .unwrap_or_else(|| "full".to_owned()),
        config
            .max_records
            .map(|value| value.to_string())
            .unwrap_or_else(|| "all".to_owned()),
        config.collect_alternate_data_streams,
        config.collect_data_run_summary,
        config.collect_dos_file_name_links,
        config.include_unused_records,
    );
}

/// Run the serial read workload and return a work-touching summary.
///
/// This iterates the raw `$MFT` serially, materializing a full
/// [`RawMftEntry`] per visible record, and folds a handful of fields into the
/// returned [`SerialSummary`].
pub fn run_serial_read(mft: &RawMft<'_>, config: &SerialConfig) -> Result<SerialSummary, UsnError> {
    let mut summary = SerialSummary::default();
    let iter = mft.try_iter_with_options(config.scan_options())?;
    let limit = config.max_records.unwrap_or(usize::MAX);
    for item in iter.take(limit) {
        match item {
            Ok(entry) => {
                summary.records += 1;
                summary.name_chars += entry.file_name.len() as u64;
                summary.links += entry.links.len() as u64;
                summary.ads += entry.alternate_data_streams.len() as u64;
                summary.size_checksum ^= entry.real_size ^ entry.allocated_size;
            }
            Err(_) => summary.errors += 1,
        }
    }
    Ok(summary)
}

/// Parse a `u64` environment variable, returning `None` when unset or invalid.
fn parse_env_u64(key: &str) -> Option<u64> {
    env::var(key).ok().and_then(|value| value.parse().ok())
}

/// Parse a `usize` environment variable, returning `None` when unset or invalid.
fn parse_env_usize(key: &str) -> Option<usize> {
    env::var(key).ok().and_then(|value| value.parse().ok())
}

/// Parse a boolean environment variable (`0`/`false`/`no` disable it).
fn parse_env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => default,
    }
}

/// Convert a `usize` into a `NonZeroUsize`, clamping zero to one.
pub fn nonzero_usize(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or(NonZeroUsize::MIN)
}
