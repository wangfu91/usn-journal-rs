# Copilot Instructions

## Build and test

- CI runs on `windows-latest`, so prefer validating changes on Windows.
- Build the crate with `cargo build`.
- Run the full test suite with `cargo test`.
- Run a single test with `cargo test <full-or-partial-test-name>`, for example `cargo test path::tests::test_resolve_path_with_cache_hit`.
- List exact test names with `cargo test -- --list`.
- Run examples with `cargo run --example read_journal`, `cargo run --example enum_mft`, `cargo run --example raw_mft`, `cargo run --example change_monitor`, or `cargo run --example pretty_print`.
- Run benchmarks with `cargo bench --bench raw_mft` (Divan harness). Available bench targets: `raw_mft`, `journal`, `path_resolver`. Filter a single bench with `cargo bench --bench raw_mft -- raw_mft_iter`. Set `USN_TEST_DRIVE` (default `C`) to pick the volume.
- Release validation also uses `cargo package` before publishing to crates.io.

## High-level architecture

- `src\lib.rs` defines the public crate surface: `volume`, `journal`, `mft`, `raw_mft`, `path`, `errors`, `types`, `time`, and the `UsnResult<T>` alias.
- `src\volume.rs` is the handle boundary. It opens raw volume handles from either a drive letter or mount point, stores the handle in `Volume`, and closes it in `Drop`. Fields are private; use accessor methods.
- `src\journal\` is the USN journal module directory (split from `src\journal.rs`). Submodules: `mod.rs`, `journal.rs` (`UsnJournal`), `iter.rs` (`UsnJournalIter`), `entry.rs` (`UsnEntry`), `reason.rs` (reason-flag lookup table), `options.rs` (`JournalIterOptions`), `data.rs`, `defaults.rs`.
- `src\mft\` is the MFT enumeration API module directory. Submodules: `mod.rs`, `mft.rs` (`Mft`), `iter.rs` (`MftIter`), `entry.rs` (`MftEntry`), `options.rs` (`MftIterOptions` with typed `UsnRecordVersion`), `tests.rs`. It wraps a `Volume`, issues `FSCTL_ENUM_USN_DATA`, and yields `MftEntry` per record. `UsnRecordVersion::V2` forces `USN_RECORD_V2` (standard 64-bit IDs); `V3` (default) permits `USN_RECORD_V3` (128-bit extended IDs, used by Windows 11 even on NTFS).
- `src\usn_record.rs` is the shared low-level parser for buffers returned by the Windows APIs. It extracts the next cursor (`USN` or file ID), validates record boundaries, and returns typed `USN_RECORD_V2` references for both journal and MFT iteration.
- `src\path.rs` is the shared path-resolution layer. It abstracts over `UsnEntry`, `MftEntry`, and `RawMftEntry` through `PathResolvableEntry`. `PathResolver::new(v)` enables an LRU directory cache by default; use `.without_lru_cache()` for uncached syscall resolution or `.with_in_memory_tree(&raw_mft)?` for O(1) resolution on large scans (no per-lookup syscalls).
- `src\raw_mft\` reads the `$MFT` file directly from the volume and parses each FILE record into a rich `RawMftEntry` (full timestamps, real / allocated size, hard link count, alternate data streams, sparse / compressed / encrypted flags, data-run summary, file-name namespace). Submodules: `boot` (boot sector geometry), `fixup` (USA verification), `io` (sector-aligned `VolumeReader`), `attribute` / `data_run` / `record` (on-disk structures), `extent` (record number → volume offset), `entry` (`RawMftEntry` builder, `AttributeListInfo`), `options` (`RawMftIterOptions`). `$ATTRIBUTE_LIST` is fully handled: resident lists are parsed inline; non-resident lists are read from disk via their data runs. Extension records are loaded and the highest-scoring file-name namespace wins (Win32AndDos > Win32 > Posix > Dos), eliminating spurious DOS 8.3 short names. **NTFS only** — ReFS volumes return `UsnError::UnsupportedFilesystem`.
- `src\types.rs` defines the `Usn(i64)` and `Fid(u64)` newtypes used throughout the public API.
- `src\time.rs` defines `Filetime(u64)` with `to_system_time`, `from_system_time`, `to_unix_seconds`, and `to_unix_nanos`.
- Supporting modules: `src\privilege.rs` checks elevation, `src\errors.rs` defines the crate-wide `UsnError` enum and `UsnResult<T>` alias.

## Key conventions

- The crate is Windows-only and targets NTFS/ReFS volumes. Real USN journal and MFT access is privilege-gated, so code paths that open volumes should continue to check elevation early.
- Keep unsafe Win32 interaction localized. Public APIs expose Rust structs and iterators, while raw buffer walking and pointer validation stay in helper code such as `usn_record.rs` and the small FFI call sites.
- Both `UsnJournalIter` and `MftIter` yield `UsnResult<_>` per item instead of failing the whole scan on a single record-level problem. Match that pattern when extending enumeration APIs.
- Reuse the shared parsing helpers in `src\usn_record.rs` for cursor extraction and record validation instead of duplicating buffer logic in `journal\` or `mft.rs`.
- Use `UsnError` and the `UsnResult<T>` alias for public fallible APIs. Prefer concrete variants (`NotElevated`, `UnsupportedFilesystem`, `BufferTooSmall`, `InvalidRecord`) over generic catch-alls.
- Use `Usn` and `Fid` newtypes in all new code; do not use bare `i64`/`u64` for these concepts.
- `Filetime` is the canonical timestamp type. Default builds must not depend on external date/time crates.
- Raw `$MFT` access (`RawMft`) is **NTFS only**. ReFS volumes return `UsnError::UnsupportedFilesystem`. Guard accordingly.
- `RawMft` fully handles `$ATTRIBUTE_LIST`: both resident and non-resident cases are parsed, extension records are loaded, and the best-namespace `$FILE_NAME` (Win32 long name) is always preferred over a DOS 8.3 short name. Do not regress this by skipping attr-list processing.
- Tests live inline in the module files under `#[cfg(test)]` rather than in a separate `tests\` tree.
- Error-path tests for Win32 calls use `injectorpp` to fake API behavior, while integration-style volume and privilege tests accept permission-related outcomes on non-elevated runs.
- When adding path-aware enumeration code, use `PathResolver::new(v)` or `.with_lru_cache(n)` for moderate scans and `.with_in_memory_tree(&raw_mft)?` for full-volume scans. Do not use the removed `PathResolver::new_with_cache`.
