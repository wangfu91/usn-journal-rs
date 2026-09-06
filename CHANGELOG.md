# Changelog

## Unreleased: raw MFT extraction

Raw MFT reading, snapshot/historical paths and their examples, tests and
benchmarks moved to the independent `ntfs-mft` repository. The journal, FSCTL
enumeration, live-path and shared-type refinements remain here. Raw-MFT entries
below describe pre-extraction branch history; use `ntfs_mft` imports and
`MftError`/`MftResult` for the extracted APIs.


All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/).

---

## [0.5.0] — Unreleased

Major version bump with extensive performance work, ergonomic API improvements,
and idiomatic Rust refactoring. **Breaking changes throughout** — see the
[migration guide](#migrating-from-04x) below.

### Highlights

- Raw NTFS `$MFT` reading now supports Linux mount points and device paths with
  read-only device access; Windows-only USN/FSCTL APIs are compile-time gated.
- Raw `$MFT` iteration is ~6× faster (262 ms vs 1.64 s for 200 k records).
- New in-memory directory-tree path resolver: full-volume scans drop from ~21 s
  to <500 ms (~40× faster).
- Timestamps now use a lightweight `Filetime(u64)` newtype instead of an
  external date/time dependency.
- Strong typing via `Usn`, `Fid`, `UsnReason`, `UsnSourceInfo`, and `FileAttributes`.
- Builder patterns for all iterator option structs.
- Concrete `UsnError` variants — no more `OtherError(String)`.

### Performance

- Linux raw-MFT filename decoding avoids an intermediate UTF-16 allocation,
  improving the measured 2.1 M-record serial workload by about 6.2%.
- Raw `$MFT` reader: zero-copy fixup parsing via `VolumeReader::borrow_at`,
  eliminated per-record memcpy.
- Path resolver: `Arc<Path>` cache values for cheap clones; reusable scratch
  buffer; in-memory directory tree.
- USN reason-string formatting via static lookup table.
- Checked unaligned-read helper for byte-oriented record parsers.
- Shared data-run decoder used by both full decode and summary-only paths.

### Added

- Linux `Volume::from_device_path`; Linux `Volume::from_mount_point` resolves
  NTFS backing devices through `/proc/self/mountinfo` and opens them read-only.
- `usn_journal_rs::types` module with `Usn` and `Fid` newtypes.
- `usn_journal_rs::time::Filetime` with `to_system_time`, `from_system_time`,
  `TryFrom` conversions, `to_unix_seconds`, and `to_unix_nanos`.
- `Volume::from_drive_letter(c: char)` and `Volume::from_mount_point(p)` —
  replaces the previous single constructor.
- `Volume` now records the originating drive letter or mount point internally.
- `JournalIterOptions::builder()`, `MftIterOptions::builder()`,
  `RawMftScanOptions::builder()`, and `RawMftChunkPlanOptions::builder()`.
- `RawMft::parallel()` builder-style facade for ordered parallel chunk scans.
- `PathResolver::new(v).with_directory_cache(n)` for tuning (or disabling, with
  `0`) the live resolver's LRU directory cache.
- `usn_journal_rs::prelude` for common application imports.
- The prelude now also re-exports the entry types (`UsnEntry`, `MftEntry`,
  `RawMftEntry`) and the option builders (`JournalIterOptions`, `MftIterOptions`,
  `RawMftScanOptions`, `UsnRecordVersion`).
- `RawMft::path_resolver()` → `RawMftPathResolver` for snapshot-local O(1) path
  reconstruction without per-lookup syscalls.
- `Volume::journal()`, `Volume::mft()`, `Volume::raw_mft()`, and
  `Volume::path_resolver()` convenience accessors for discovering the API from a
  `Volume` (e.g. `volume.journal().try_iter()?`).
- USN v3 / 128-bit file ID support for `UsnJournal`, `Mft`, and `PathResolver`.
- `UsnError::NotElevated`, `UsnError::UnsupportedFilesystem(String)`,
  `UsnError::BufferTooSmall { needed, got }`, precise USN parser error
  variants, and offset-aware raw-MFT parse diagnostics.
- `UsnError::JournalNotActive` plus `UsnJournal::query_or_create()` — explicit
  "query, creating the journal only if it is missing" semantics.
- `UsnReason::ALL` — a strongly-typed full 32-bit catch-all reason mask that also
  matches reason bits newer than this crate (unlike bitflags' `UsnReason::all()`).
- Named predicate methods on `UsnReason` (`is_file_create`, `is_file_delete`,
  `is_rename`, `is_close`, `is_data_change`) and on `FileAttributes`
  (`is_directory`, `is_read_only`, `is_hidden`, `is_system`, `is_archive`,
  `is_reparse_point`, `is_compressed`, `is_encrypted`, `is_sparse`, `is_offline`,
  `is_temporary`) for readable change/attribute classification.
- `UsnEntry` and `MftEntry` gained matching attribute convenience methods
  (`is_read_only`, `is_system`, `is_archive`, `is_reparse_point`, `is_compressed`,
  `is_encrypted`, `is_sparse`) alongside the existing `is_dir` / `is_hidden`.
- `Usn::ZERO` constant for the conventional scan-start cursor.
- `Filetime::UNIX_EPOCH` constant, `Filetime::is_zero()`,
  `Filetime::to_unix_millis()`, and `Filetime::from_unix_seconds()`
  (the missing inverse of `to_unix_seconds`).
- `Fid::from_parts(record_number, sequence)` — construct a standard NTFS file
  reference from its parts (inverse of `record_number()` / `sequence()`).
- `Usn::is_zero()`, `Usn::saturating_add(i64)`, and `Usn::checked_add(i64)`.
- `fmt::LowerHex` / `fmt::UpperHex` for `Fid` so `{:x}` / `{:X}` / `{:#x}` compose.
- `Display` for `UsnJournalData` (compact one-line summary for logging).
- `Display` impl on `UsnEntry` and `MftEntry` (compact one-line format).
- `Display` impl on `RawMftEntry` (compact one-line format), for parity with the
  journal and FSCTL-based MFT entries.
- Runnable doc examples on `Fid::from_parts`, `Filetime::from_unix_seconds`, and
  `Volume::from_drive_letter`.
- Benchmarks: `benches/journal.rs`, `benches/path_resolver.rs`.
- Integration tests: `tests/journal_query.rs` (query / query_or_create
  semantics), `tests/volume_accessors.rs` (Volume accessor equivalence + MFT
  USN-range edge cases), `tests/path_resolver_consistency.rs`,
  `tests/raw_mft_mft_consistency.rs`, `tests/refs_unsupported.rs`,
  `tests/refs_v3_ids.rs`, and `tests/filetime_roundtrip.rs`.

### Changed

- `Volume` fields are now private; use the public accessor methods.
- `PathResolver::new` now enables the default LRU directory cache automatically;
  call `.with_directory_cache(0)` for fully uncached syscall resolution.
- `RawMftEntry` timestamps are `Filetime` instead of external date/time types.
- `UsnEntry::time` is `Filetime` instead of `std::time::SystemTime`.
- `RawMftIterOptions` renamed to `RawMftScanOptions`, with structured
  `RawMftReadBuffers`, `RawMftRecordRange`, and `RawMftEntryOptions` groups.
- `RawMftWorkPlanOptions` renamed to `RawMftChunkPlanOptions`.
- `RawMft` now uses the same fallible iterator naming as the journal and
  FSCTL-based MFT APIs: `try_iter` / `try_iter_with_options`.
- `RawMft::get_record` renamed to `RawMft::read_record`.
- `journal::EnumOptions` renamed to `JournalIterOptions`;
  `mft::EnumOptions` renamed to `MftIterOptions`.
- `UsnJournal` and `Mft` fallible iteration entry points are now `try_iter` /
  `try_iter_with_options`.
- `PathResolvableEntry::fid()` and `parent_fid()` now return `Fid`.
- `Volume` keeps the originating drive letter or mount point internally; use `Volume::drive_letter()` and `Volume::mount_point()` to inspect it.
- Entry structs now derive `Clone`, `PartialEq`, `Eq`, and `Hash` where their
  field types permit it.
- `Fid` now represents both standard 64-bit NTFS file references and
  128-bit ReFS file IDs. Use `is_standard()`, `is_extended()`, `as_u64()`,
  `as_u128()`, and `as_bytes()` to inspect the underlying representation.
- `src/journal.rs` split into the `src/journal/` module directory
  (`mod.rs`, `journal.rs`, `iter.rs`, `entry.rs`, `reason.rs`, `options.rs`,
  `data.rs`, `defaults.rs`).
- `src/record.rs` renamed to `src/usn_record.rs`.
- Raw `$MFT` on-disk parser modules live under `src/raw_mft/layout/`, and entry
  construction was split into focused attribute dispatch, capture, and file-name
  selection modules under `src/raw_mft/entry_build/`.
- Cargo profile cleanup: removed bogus `[profile.test]` flags; added
  `[profile.bench] lto = "thin"`.
- `UsnJournal::query` no longer takes a `create_if_not_active: bool`. It now
  queries only and returns `UsnError::JournalNotActive` when the volume has no
  journal; use the new `UsnJournal::query_or_create()` to create on demand.
- `PathResolver::resolve_path` now takes `&self` instead of `&mut self`
  (directory cache and scratch buffer use interior mutability), matching
  `RawMftPathResolver::resolve_path`. A single resolver can be shared across an
  iteration loop without a `mut` binding.
- `UsnReason`, `FileAttributes`, and `UsnSourceInfo` now share one `Display`
  implementation. An empty `UsnReason` renders as `NONE` (previously `UNKNOWN`),
  and a value with only unknown bits renders as hex (e.g. `0x8`), consistent
  with the other two bitflag types.
- `UsnError::InvalidMountPointError` renamed to `UsnError::InvalidMountPoint`
  (the redundant `Error` suffix is dropped).
- `Mft::try_iter_with_options` no longer seeds the enumeration's start file
  reference number from `low_usn`; enumeration always begins at record 0 and
  `low_usn`/`high_usn` filter purely by USN, as the Win32 API intends.
- `JournalIterOptionsBuilder::timeout` now takes a `std::time::Duration`
  (truncated to whole seconds, matching the Win32 read API) instead of a raw
  `u64`. The default is `Duration::ZERO` (block indefinitely while waiting).

### Removed

- `pub type Usn = i64` alias (replaced by the `Usn(i64)` newtype).
- `UsnEntry::pretty_format` and `MftEntry::pretty_format` — use the `Display`
  impl; a multi-line formatter is available in `examples/journal_pretty_print.rs`.
- `UsnEntry::get_reason_string()` — use the `UsnReason` `Display` impl directly,
  e.g. `entry.reason.to_string()` or `format!("{}", entry.reason)`.
- `UsnError::OtherError(String)` catch-all variant.
- `PathResolver::new_with_cache` — use
  `PathResolver::new(v).with_directory_cache(n)`.
- `Fid::from_u64`; use `Fid::new` or `Fid::from(u64)` instead.
- External date/time crate integration from the public API.
- Crate-root re-exports of `DEFAULT_JOURNAL_MAX_SIZE`,
  `DEFAULT_JOURNAL_ALLOCATION_DELTA`, `USN_REASON_MASK_ALL`, and
  `DEFAULT_BUFFER_SIZE` (moved into the `journal` module).

### Internal

- `src/record.rs` renamed to `src/usn_record.rs`.
- Unified the `FileAttributes` / `UsnReason` / `UsnSourceInfo` `Display` logic
  into a single `crate::display::write_flag_names` helper.
- Named `attr_header_flags` constants replace magic-number attribute-flag checks
  in raw-`$MFT` entry construction; a shared `parse_record_at` helper removes the
  duplicated look-up/borrow/validate/fix-up sequence in the raw-`$MFT` readers.

---

## Migrating from 0.4.x

### Volume construction

```diff
- let volume = Volume::new(Some('C'), None)?;
+ let volume = Volume::from_drive_letter('C')?;
```

Or via mount point:

```diff
- let volume = Volume::new(None, Some(r"C:\"))?;
+ let volume = Volume::from_mount_point(r"C:\")?;
```

### Iterating the USN journal

```diff
- for entry in journal.iter()? {
+ for entry in journal.try_iter()? {
```

### Iterating the MFT

```diff
- for entry in mft.iter()? {
+ for entry in mft.try_iter()? {
```

### Iterator options

```diff
- use usn_journal_rs::journal::EnumOptions;
- let opts = EnumOptions { start_usn: 0, ..Default::default() };
+ use usn_journal_rs::journal::JournalIterOptions;
+ use std::num::NonZeroUsize;
+ use usn_journal_rs::{Usn, UsnReason};
+ let opts = JournalIterOptions::builder()
+     .start_usn(Usn::new(0))
+     .reason_mask(UsnReason::ALL)
+     .buffer_bytes(NonZeroUsize::new(64 * 1024).unwrap())
+     .build();
```

### Timestamps

```diff
- let dt = entry.created;
+ use usn_journal_rs::time::Filetime;
+ let ft: Filetime = entry.time;
+ let st: Option<std::time::SystemTime> = ft.to_system_time();
+ let ft2 = Filetime::from_system_time(std::time::SystemTime::now());
+ let unix: i64 = ft.to_unix_seconds();
```

### Error matching

```diff
- match err {
-     UsnError::OtherError(msg) => eprintln!("error: {msg}"),
-     _ => {}
- }
+ match err {
+     UsnError::Io(e)                     => eprintln!("I/O error: {e}"),
+     UsnError::WinApi(e)                 => eprintln!("Win32 error: {e}"),
+     UsnError::NotElevated               => eprintln!("must be Administrator"),
+     UsnError::UnsupportedFilesystem(fs) => eprintln!("not supported on {fs}"),
+     UsnError::BufferTooSmall { needed, got } => eprintln!("buffer too small: need {needed}, got {got}"),
+     UsnError::InvalidRecord { offset, reason } => eprintln!("bad record at {offset}: {reason}"),
+     _ => {}
+ }
```

### Path resolver

```diff
- let resolver = PathResolver::new_with_cache(&volume, 8192);
+ // Plain integer capacity; pass 0 to disable the cache.
+ let resolver = PathResolver::new(&volume).with_directory_cache(8192);
```

For maximum performance on full-volume scans, use the raw-`$MFT` snapshot
resolver, which walks an in-memory directory tree with no per-lookup syscalls:

```rust
use usn_journal_rs::raw_mft::RawMft;

let raw_mft = RawMft::new(&volume)?;
let resolver = raw_mft.path_resolver()?;
```

---

## Earlier versions

See git history for 0.4.x and prior.
