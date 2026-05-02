# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/).

---

## [0.5.0] — Unreleased

Major version bump with extensive performance work, ergonomic API improvements,
and idiomatic Rust refactoring. **Breaking changes throughout** — see the
[migration guide](#migrating-from-04x) below.

### Highlights

- Raw `$MFT` iteration is ~6× faster (262 ms vs 1.64 s for 200 k records).
- New in-memory directory-tree path resolver: full-volume scans drop from ~21 s
  to <500 ms (~40× faster).
- Timestamps now use a lightweight `Filetime(u64)` newtype instead of an
  external date/time dependency.
- Strong typing via `Usn(i64)` and `Fid` typed file IDs (64-bit NTFS + 128-bit ReFS).
- Builder patterns for all iterator option structs.
- Concrete `UsnError` variants — no more `OtherError(String)`.

### Performance

- Raw `$MFT` reader: zero-copy fixup parsing via `VolumeReader::borrow_at`,
  eliminated per-record memcpy.
- Path resolver: `Arc<Path>` cache values for cheap clones; reusable scratch
  buffer; in-memory directory tree.
- USN reason-string formatting via static lookup table.
- `read_unaligned` for unaligned `i64`/`u64` reads in record parsers.

### Added

- `usn_journal_rs::types` module with `Usn` and `Fid` newtypes.
- `usn_journal_rs::time::Filetime` with `to_system_time`, `to_unix_seconds`,
  and `to_unix_nanos`.
- `Volume::from_drive_letter(c: char)` and `Volume::from_mount_point(p)` —
  replaces the previous single constructor.
- `VolumeSource` enum (`DriveLetter` vs `MountPoint`).
- `JournalIterOptions::builder()`, `MftIterOptions::builder()`,
  `RawMftIterOptions::builder()`.
- `PathResolver::new(v).with_lru_cache(n).with_in_memory_tree(&raw_mft)?`
  fluent builder API.
- `InMemoryDirTree::from_raw_mft` for O(1) path resolution without per-lookup
  syscalls.
- USN v3 / 128-bit file ID support for `UsnJournal`, `Mft`, and `PathResolver`.
- `UsnError::NotElevated`, `UsnError::UnsupportedFilesystem(String)`,
  `UsnError::BufferTooSmall { needed, got }`, and
  `UsnError::InvalidRecord { offset, reason }` variants.
- `Display` impl on `UsnEntry` and `MftEntry` (compact one-line format).
- Benchmarks: `benches/journal.rs`, `benches/path_resolver.rs`.
- Integration tests: `tests/in_memory_tree.rs`,
  `tests/path_resolver_consistency.rs`, `tests/refs_unsupported.rs`,
  `tests/filetime_roundtrip.rs`.

### Changed

- `Volume` fields are now private; use the public accessor methods.
- `PathResolver::new` now enables the default LRU directory cache automatically;
  call `.without_lru_cache()` for fully uncached syscall resolution.
- `RawMftEntry` timestamps are `Filetime` instead of external date/time types.
- `UsnEntry::time` is `Filetime` instead of `std::time::SystemTime`.
- `RawMftIterOptions::batch_records: usize` →
  `RawMftIterOptions::buffer_bytes: NonZeroUsize`.
- `journal::EnumOptions` renamed to `JournalIterOptions`;
  `mft::EnumOptions` renamed to `MftIterOptions`.
- Fallible iteration entry points renamed to `try_iter` / `try_iter_with_options`.
- `PathResolvableEntry::fid()` and `parent_fid()` now return `Fid`.
- `Fid` now represents both standard 64-bit NTFS file references and
  128-bit ReFS file IDs. Use `is_standard()`, `is_extended()`, `as_u64()`,
  `as_u128()`, and `as_bytes()` to inspect the underlying representation.
- `src/journal.rs` split into the `src/journal/` module directory
  (`mod.rs`, `journal.rs`, `iter.rs`, `entry.rs`, `reason.rs`, `options.rs`,
  `data.rs`, `defaults.rs`).
- `src/record.rs` renamed to `src/usn_record.rs` to disambiguate from
  `src/raw_mft/record.rs`.
- Cargo profile cleanup: removed bogus `[profile.test]` flags; added
  `[profile.bench] lto = "thin"`.

### Removed

- `pub type Usn = i64` alias (replaced by the `Usn(i64)` newtype).
- `UsnEntry::pretty_format` and `MftEntry::pretty_format` — use the `Display`
  impl; a multi-line formatter is available in `examples/pretty_print.rs`.
- `UsnError::OtherError(String)` catch-all variant.
- `PathResolver::new_with_cache` (deprecated; use
  `PathResolver::new(v).with_lru_cache(n)`).
- External date/time crate integration from the public API.
- Crate-root re-exports of `DEFAULT_JOURNAL_MAX_SIZE`,
  `DEFAULT_JOURNAL_ALLOCATION_DELTA`, `USN_REASON_MASK_ALL`, and
  `DEFAULT_BUFFER_SIZE` (moved into the `journal` module).

### Internal

- `src/record.rs` renamed to `src/usn_record.rs`.

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

### Iterator options

```diff
- use usn_journal_rs::journal::EnumOptions;
- let opts = EnumOptions { start_usn: 0, ..Default::default() };
+ use usn_journal_rs::journal::{JournalIterOptions, USN_REASON_MASK_ALL};
+ use std::num::NonZeroUsize;
+ use usn_journal_rs::{Usn, UsnReason};
+ let opts = JournalIterOptions::builder()
+     .start_usn(Usn::new(0))
+     .reason_mask(UsnReason::from_bits_retain(USN_REASON_MASK_ALL))
+     .buffer_bytes(NonZeroUsize::new(64 * 1024).unwrap())
+     .build();
```

### Timestamps

```diff
- let dt = entry.created;
+ use usn_journal_rs::time::Filetime;
+ let ft: Filetime = entry.time;
+ let st: Option<std::time::SystemTime> = ft.to_system_time();
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
+ use std::num::NonZeroUsize;
+ let resolver = PathResolver::new(&volume)
+     .with_lru_cache(NonZeroUsize::new(8192).unwrap());
```

For maximum performance on full-volume scans, use the in-memory tree:

```rust
use usn_journal_rs::raw_mft::RawMft;

let raw_mft = RawMft::new(&volume)?;
let resolver = PathResolver::new(&volume)
    .with_in_memory_tree(&raw_mft)?;
```

---

## Earlier versions

See git history for 0.4.x and prior.
