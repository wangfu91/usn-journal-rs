# Copilot Instructions

## Build and test

- CI runs on `windows-latest`, so prefer validating changes on Windows.
- Build the crate with `cargo build`.
- Run the full test suite with `cargo test`.
- Run a single test with `cargo test <full-or-partial-test-name>`, for example `cargo test path::tests::test_resolve_path_with_cache_hit`.
- List exact test names with `cargo test -- --list`.
- Run examples with `cargo run --example read_journal`, `cargo run --example enum_mft`, or `cargo run --example change_monitor`.
- Release validation also uses `cargo package` before publishing to crates.io.

## High-level architecture

- `src\lib.rs` defines the public crate surface: `volume`, `journal`, `mft`, `path`, `errors`, shared constants, and the `UsnResult<T>` alias.
- `src\volume.rs` is the handle boundary. It opens raw volume handles from either a drive letter or mount point, stores the handle in `Volume`, and closes it in `Drop`.
- `src\journal.rs` and `src\mft.rs` are the two main APIs. Both wrap a `Volume`, issue `DeviceIoControl` calls for the relevant FSCTL operations, keep iteration state in an iterator struct, and yield parsed entries as Rust iterators.
- `src\record.rs` is the shared low-level parser for buffers returned by the Windows APIs. It extracts the next cursor (`USN` or file ID), validates record boundaries, and returns typed `USN_RECORD_V2` references for both journal and MFT iteration.
- `src\path.rs` is the shared path-resolution layer. It abstracts over `UsnEntry` and `MftEntry` through `PathResolvableEntry`, and `PathResolver::new_with_cache` adds an LRU cache of directory file IDs for large scans.
- Supporting modules are narrow and focused: `src\privilege.rs` checks elevation, `src\time.rs` converts FILETIME values, and `src\errors.rs` defines the crate-wide error type.

## Key conventions

- The crate is Windows-only and targets NTFS/ReFS volumes. Real USN journal and MFT access is privilege-gated, so code paths that open volumes should continue to check elevation early.
- Keep unsafe Win32 interaction localized. Public APIs expose Rust structs and iterators, while raw buffer walking and pointer validation stay in helper code such as `record.rs` and the small FFI call sites.
- Both `UsnJournalIter` and `MftIter` yield `UsnResult<_>` per item instead of failing the whole scan on a single record-level problem. Match that pattern when extending enumeration APIs.
- Reuse the shared parsing helpers in `src\record.rs` for cursor extraction and record validation instead of duplicating buffer logic in `journal.rs` or `mft.rs`.
- Use `UsnError` and the `UsnResult<T>` alias for public fallible APIs.
- Tests live inline in the module files under `#[cfg(test)]` rather than in a separate `tests\` tree.
- Error-path tests for Win32 calls use `injectorpp` to fake API behavior, while integration-style volume and privilege tests accept permission-related outcomes on non-elevated runs.
- When adding path-aware enumeration code, prefer `PathResolver::new_with_cache` for long-running scans and preserve the current cache behavior that stores directory paths and invalidates stale entries on name mismatch.
