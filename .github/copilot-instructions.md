# Copilot Instructions

## Build and test

- CI runs on `windows-latest`, so prefer validating changes on Windows.
- Build the crate with `cargo build`.
- Run the full test suite with `cargo test`.
- Run a single test with `cargo test <full-or-partial-test-name>`, for example `cargo test path::tests::test_resolve_path_with_cache_hit`.
- List exact test names with `cargo test -- --list`.
- Release validation also uses `cargo package` before publishing to crates.io.
- Run examples with:
  - `cargo run --example read_journal`
  - `cargo run --example enum_mft`
  - `cargo run --example raw_mft_serial_read -- C`
  - `cargo run --example raw_mft_parallel_chunks`
  - `cargo run --example deletion_forensic -- C`
  - `cargo run --example change_monitor`
  - `cargo run --example journal_pretty_print`
  - profiling examples: `cargo run --example raw_mft_serial_read_profile` and `cargo run --example raw_mft_parallel_ingest_profile`
- Benchmarks use Criterion. Available bench targets are `raw_mft`, `raw_mft_ingest`, `journal`, and `path_resolver`.
- Useful benchmark commands:
  - `cargo bench --bench raw_mft -- --sample-size 20`
  - `cargo bench --bench raw_mft_ingest -- --sample-size 10`
  - `cargo bench --bench journal`
  - `cargo bench --bench path_resolver`
- Common environment variables:
  - `USN_TEST_DRIVE` selects the main target volume (default `C`).
  - `USN_REFS_TEST_DRIVE` selects the ReFS coverage volume.
  - `BENCH_RECORD_LIMIT` caps journal benchmark work.
  - `USN_RAW_MFT_SERIAL_MAX_RECORDS` caps serial raw-MFT benchmark work.
  - `USN_RAW_MFT_BENCH_*` tunes the parallel ingest benchmark.

## High-level architecture

- `src\lib.rs` defines the public crate surface: `errors`, `journal`, `mft`, `path`, `privilege`, `raw_mft`, `types`, `volume`, the `Filetime` re-export, and the `UsnResult<T>` alias.
- `src\volume.rs` is the handle boundary. It opens raw volume handles from a drive letter or mount point, stores the handle in `Volume`, closes it in `Drop`, and exposes `drive_letter()` / `mount_point()` accessors.
- `src\journal\` is the USN journal module directory. Submodules: `mod.rs`, `journal.rs` (`UsnJournal`), `iter.rs` (`UsnJournalIter`), `entry.rs` (`UsnEntry`), `reason.rs`, `options.rs` (`JournalIterOptions`), `data.rs`, and `defaults.rs`.
- `src\mft\` is the FSCTL-based MFT enumeration module. Submodules: `mod.rs`, `mft.rs` (`Mft`), `iter.rs` (`MftIter`), `entry.rs` (`MftEntry`), and `options.rs` (`MftIterOptions`, `UsnRecordVersion`). `UsnRecordVersion::V2` forces 64-bit NTFS IDs; `V3` permits 128-bit extended IDs.
- `src\usn_record.rs` is the shared low-level parser for buffers returned by the Windows journal and MFT APIs.
- `src\path\` is the live path-resolution layer. `PathResolver::new(&volume)` enables a directory cache by default; use `.with_directory_cache(n)` to resize it and `.with_directory_cache(0)` to disable it.
- `src\raw_mft\` reads the `$MFT` directly on NTFS volumes. Key internals include `layout\`, `reader.rs`, `io.rs`, `bootstrap\`, `entry_build\`, `attr_list.rs`, `history.rs`, `serial\`, `parallel\`, `chunk_plan.rs`, `options.rs`, and `path_resolver.rs`.
- `RawMft::path_resolver()` creates the snapshot-local raw-MFT path resolver. `RawMftPathResolver::with_live_fallback()` opt-in enables live fallback when snapshot lookup misses.
- `src\types.rs` defines `Usn(i64)`, the `Fid` enum for standard and extended file IDs, and the public bitflag types.
- `src\time.rs` defines `Filetime(u64)` with system-time and Unix-time conversions.
- `src\errors.rs` defines the non-exhaustive `UsnError` enum.
- Tests live in both inline `#[cfg(test)]` modules under `src\` and integration tests under `tests\`.

## Key conventions

- The crate is Windows-only and targets NTFS/ReFS volumes. Code paths that open volumes should keep checking elevation early.
- Journal and FSCTL-based MFT APIs support both NTFS and ReFS. Raw `$MFT` access is NTFS-only and should continue returning `UsnError::UnsupportedFilesystem` on unsupported filesystems.
- Keep unsafe Win32 interaction localized. Public APIs should stay in safe Rust, while raw buffer walking and pointer validation remain in helpers such as `src\usn_record.rs` and the existing raw-MFT internals.
- Reuse the shared parsing helpers in `src\usn_record.rs` instead of duplicating buffer logic in `journal\` or `mft\`.
- Use `UsnError` and the `UsnResult<T>` alias for public fallible APIs. Prefer concrete variants such as `NotElevated`, `UnsupportedFilesystem`, `BufferTooSmall`, and `InvalidRecord`.
- Use the strong domain types (`Usn`, `Fid`, `Filetime`, `FileAttributes`, `UsnReason`) instead of bare integers when the semantics matter.
- `PathResolver` is for live path resolution of `UsnEntry` and `MftEntry`. Use `RawMft::path_resolver()` for snapshot-local raw-MFT path reconstruction.
- `raw_mft::history::HistoricalPathIndex` is the best-effort helper for deleted or historical path reconstruction from one raw snapshot.
- Fallible iteration is per-item. `UsnJournal`, `Mft`, and raw-MFT iterators should keep yielding `UsnResult<_>` entries instead of failing the entire scan on one record-level problem.
- Many integration tests and benchmarks depend on Administrator privileges and suitable NTFS/ReFS volumes; they should skip gracefully when the environment is unsuitable.
- For performance claims, prefer the Criterion benchmarks over the profiling examples.
