# Agent Guide

## Project summary

- `usn-journal-rs` reads the Windows USN journal and FSCTL MFT APIs, and supports raw NTFS `$MFT` parsing on both Windows and Linux.
- Portable public modules include `errors`, `raw_mft`, `types`, `volume`, and snapshot path support. `journal`, FSCTL `mft`, `privilege`, and live path APIs are compiled only on Windows.
- Windows volume APIs require Administrator privileges. Linux raw `$MFT` access requires read permission for the backing device and is strictly read-only. Journal and FSCTL-based MFT APIs target Windows NTFS/ReFS; raw `$MFT` support is NTFS-only.

## Build, test, examples, and packaging

- Build with `cargo build`.
- Run the full test suite with `cargo test`.
- List exact test names with `cargo test -- --list`.
- Validate the publishable package with `cargo package`.
- Main examples:
  - `cargo run --features windows-examples --example read_journal` (Windows)
  - `cargo run --features windows-examples --example enum_mft` (Windows)
  - `cargo run --example raw_mft_serial_read -- C`
  - `cargo run --example raw_mft_parallel_chunks`
  - `cargo run --example deletion_forensic -- C`
  - On Linux, pass an NTFS mount or device path instead of `C`, for example `cargo run --example deletion_forensic -- /media/user/windows`.
  - `cargo run --features windows-examples --example change_monitor` (Windows)
  - `cargo run --features windows-examples --example journal_pretty_print` (Windows)
- Profiling-oriented examples:
  - `cargo run --example raw_mft_serial_read_profile`
  - `cargo run --example raw_mft_parallel_ingest_profile`

## Benchmarks

- Benchmarks use Criterion. Available bench targets are `raw_mft`, `raw_mft_ingest`, `journal`, and `path_resolver`.
- Typical commands:
  - `cargo bench --bench raw_mft -- --sample-size 20`
  - `cargo bench --bench raw_mft_ingest -- --sample-size 10`
  - `cargo bench --features windows-examples --bench journal` (Windows)
  - `cargo bench --features windows-examples --bench path_resolver` (Windows)
- Common environment variables:
  - `USN_TEST_DRIVE` selects the main target volume (default `C`).
  - `USN_TEST_VOLUME` selects a Linux NTFS mount or device path for raw-MFT benchmarks.
  - `USN_REFS_TEST_DRIVE` selects the ReFS coverage volume (default `D` in current tests).
  - `BENCH_RECORD_LIMIT` caps journal benchmark iteration count.
  - `USN_RAW_MFT_SERIAL_MAX_RECORDS` caps serial raw-MFT bench work.
  - `USN_RAW_MFT_BENCH_*` variables tune the parallel ingest benchmark.

## Code map

- `src\volume.rs` owns the platform volume source: a Win32 handle from a drive/mount point or a read-only Linux device file resolved from `from_mount_point` / `from_device_path`.
- `src\journal\` contains the USN journal API: `UsnJournal`, `UsnJournalIter`, `UsnEntry`, `UsnJournalData`, `JournalIterOptions`, and defaults such as `USN_REASON_MASK_ALL`.
- `src\mft\` contains the FSCTL-based MFT enumerator: `Mft`, `MftIter`, `MftEntry`, `MftIterOptions`, and `UsnRecordVersion`.
- `src\path\` is the live path-resolution layer. `PathResolver::new(&volume)` enables a directory cache by default; tune or disable it with `.with_directory_cache(n)` where `0` disables caching.
- `src\raw_mft\` is the NTFS-only raw reader. Important internals live in `layout\`, `reader.rs`, `io.rs`, `bootstrap\`, `entry_build\`, `attr_list.rs`, `history.rs`, `serial\`, `parallel\`, `chunk_plan.rs`, `options.rs`, and `path_resolver.rs`.
- `src\raw_mft\README.md` documents the internal raw-MFT read flow. `docs\raw-mft-read.md` and `docs\raw_mft_parallel_ingest_findings.md` contain additional implementation and performance notes.
- `src\usn_record.rs` is the shared low-level parser for USN buffers returned by the Windows APIs.
- `src\types.rs` defines the strong public types, including `Usn(i64)`, `Fid`, `FileAttributes`, `UsnReason`, and `UsnSourceInfo`.
- `src\time.rs` defines `Filetime(u64)`.
- `src\errors.rs` defines the non-exhaustive `UsnError` enum.
- `tests\` contains integration coverage alongside the inline `#[cfg(test)]` module tests under `src\`.

## Current API notes

- `Fid` is an enum, not a plain `u64`: `Fid::Standard(u64)` for standard NTFS file references and `Fid::Extended(u128)` for USN v3 / ReFS-style 128-bit IDs.
- Live path resolution for `UsnEntry` and `MftEntry` goes through `PathResolver`.
- Snapshot-local raw-MFT path resolution goes through `RawMft::path_resolver()`. Windows additionally exposes `.with_live_fallback()` for intentional `OpenFileById` fallback; Linux remains snapshot-only.
- Parallel raw-MFT work is driven by `RawMft::parallel()`, `RawMftChunkPlanOptions`, and `RawMftParallelScheduling`.
- `raw_mft::history::HistoricalPathIndex` is the best-effort helper for deleted or historical path reconstruction from a raw snapshot.

## Working conventions

- Keep unsafe Win32 interaction localized to the existing FFI boundaries and reuse the shared parsing helpers instead of duplicating buffer-walking logic.
- Use `UsnResult<T>` and concrete `UsnError` variants for public fallible APIs.
- Preserve the strong domain types (`Usn`, `Fid`, `Filetime`, `FileAttributes`, `UsnReason`) instead of falling back to primitive integers in public-facing code.
- Fallible iteration is item-based: journal, FSCTL MFT, and raw-MFT iterators yield `UsnResult<_>` per entry instead of failing the entire scan on one bad record.
- Volume-, filesystem-, and privilege-dependent tests and benches are expected to skip gracefully when the environment is unsuitable.
- For performance claims, prefer the Criterion benchmarks over the profiling examples.
