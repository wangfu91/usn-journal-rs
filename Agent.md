# Agent Guide

## Project summary

- `usn-journal-rs` is a Windows-only Rust crate for reading the USN change journal, enumerating the MFT through the FSCTL APIs, and parsing the raw NTFS `$MFT` for richer metadata.
- Public crate modules are rooted in `src\lib.rs`: `errors`, `journal`, `mft`, `path`, `privilege`, `raw_mft`, `types`, `volume`, plus the re-exported `Filetime` type and `UsnResult<T>` alias.
- Opening a volume requires Administrator privileges. Journal and FSCTL-based MFT APIs target NTFS and ReFS; raw `$MFT` support is NTFS-only and returns `UsnError::UnsupportedFilesystem` on unsupported filesystems.

## Build, test, examples, and packaging

- Build with `cargo build`.
- Run the full test suite with `cargo test`.
- List exact test names with `cargo test -- --list`.
- Validate the publishable package with `cargo package`.
- Main examples:
  - `cargo run --example read_journal`
  - `cargo run --example enum_mft`
  - `cargo run --example raw_mft_serial_read -- C`
  - `cargo run --example raw_mft_parallel_chunks`
  - `cargo run --example deletion_forensic -- C`
  - `cargo run --example change_monitor`
  - `cargo run --example journal_pretty_print`
- Profiling-oriented examples:
  - `cargo run --example raw_mft_serial_read_profile`
  - `cargo run --example raw_mft_parallel_ingest_profile`

## Benchmarks

- Benchmarks use Criterion. Available bench targets are `raw_mft`, `raw_mft_ingest`, `journal`, and `path_resolver`.
- Typical commands:
  - `cargo bench --bench raw_mft -- --sample-size 20`
  - `cargo bench --bench raw_mft_ingest -- --sample-size 10`
  - `cargo bench --bench journal`
  - `cargo bench --bench path_resolver`
- Common environment variables:
  - `USN_TEST_DRIVE` selects the main target volume (default `C`).
  - `USN_REFS_TEST_DRIVE` selects the ReFS coverage volume (default `D` in current tests).
  - `BENCH_RECORD_LIMIT` caps journal benchmark iteration count.
  - `USN_RAW_MFT_SERIAL_MAX_RECORDS` caps serial raw-MFT bench work.
  - `USN_RAW_MFT_BENCH_*` variables tune the parallel ingest benchmark.

## Code map

- `src\volume.rs` owns raw volume handles opened through `Volume::from_drive_letter` or `Volume::from_mount_point`, and exposes `drive_letter()` / `mount_point()` accessors.
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
- Snapshot-local raw-MFT path resolution goes through `RawMft::path_resolver()`, which returns `RawMftPathResolver`. Call `.with_live_fallback()` only when you intentionally want live `OpenFileById` fallback mixed into snapshot results.
- Parallel raw-MFT work is driven by `RawMft::parallel()`, `RawMftChunkPlanOptions`, and `RawMftParallelScheduling`.
- `raw_mft::history::HistoricalPathIndex` is the best-effort helper for deleted or historical path reconstruction from a raw snapshot.

## Working conventions

- Keep unsafe Win32 interaction localized to the existing FFI boundaries and reuse the shared parsing helpers instead of duplicating buffer-walking logic.
- Use `UsnResult<T>` and concrete `UsnError` variants for public fallible APIs.
- Preserve the strong domain types (`Usn`, `Fid`, `Filetime`, `FileAttributes`, `UsnReason`) instead of falling back to primitive integers in public-facing code.
- Fallible iteration is item-based: journal, FSCTL MFT, and raw-MFT iterators yield `UsnResult<_>` per entry instead of failing the entire scan on one bad record.
- Volume-, filesystem-, and privilege-dependent tests and benches are expected to skip gracefully when the environment is unsuitable.
- For performance claims, prefer the Criterion benchmarks over the profiling examples.
