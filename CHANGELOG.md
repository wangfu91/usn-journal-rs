# Changelog

## [Unreleased] — 0.5.0

API refinements for Windows USN journals, FSCTL enumeration, and live paths.
This release is not yet published. The 0.4.1 correctness fixes and shared
handle ownership are retained.

### Added

- Strong `Usn`, `Fid`, `FileAttributes`, `UsnReason`, and `UsnSourceInfo`
  types, including 128-bit file IDs for V3/ReFS records.
- `Filetime` at the crate root, with checked `SystemTime` conversions.
- Validated `JournalIterOptions` and `MftIterOptions` builders, typed
  `UsnRecordVersion`, and a common `prelude`.
- `UsnJournal::query_or_create()` for explicit create-on-demand queries.
- `PathResolver::try_resolve_path()` preserves OS lookup errors;
  `resolve_path()` remains an optional-result convenience method.
- `Mft::iter_with_buffer()` and `UsnJournal::try_iter_with_buffer()` accept
  reusable buffers during construction, without allocating a replacement first.
- `UsnError::is_permission_denied()` recognizes the permission variant and
  underlying I/O/Win32 permission failures while preserving OS error sources.
- Compact `Display` formatting, flag predicates, benchmarks, and regression tests.

### Changed

- Journal `iter()` and `try_iter()` use the same non-waiting default options.
  Enable `wait_for_more(true)` explicitly for monitoring.
- `query()` only queries and returns `UsnError::JournalNotActive` when absent.
  Iterator construction retains create-on-demand behavior.
- Options builders return `UsnResult<Options>`: buffer lengths must fit the
  8-byte cursor and Win32 u32 length; MFT lower bounds must not exceed upper bounds.
- `timeout(Duration)` rounds positive fractional seconds up. Zero remains an
  indefinite kernel wait when waiting is enabled; this is not a deadline for next().
- `PathResolver::new()` remains uncached; `with_directory_cache(n)` configures
  caching for stable trees. `resolve_path()` accepts `&self`.
- Volume metadata is private and exposed through accessors; mount points use
  `Path` rather than lossy string metadata.
- Entry timestamps, identifiers, and flag fields use domain types.
- Structured parser errors replace generic `OtherError` diagnostics.
- Journal, MFT, and path implementations live in module directories.

### Migration from 0.4.1

| 0.4.1 API | 0.5.0 API |
| --- | --- |
| `journal::EnumOptions { ... }` | `JournalIterOptions::builder()...build()?` |
| `mft::EnumOptions { ... }` | `MftIterOptions::builder()...build()?` |
| `journal.query(false)` | `journal.query()` |
| `journal.query(true)` | `journal.query_or_create()` |
| `volume.drive_letter` / `volume.mount_point` | `drive_letter() -> Option<char>` / `mount_point() -> Option<&Path>` |
| Integer USNs and file IDs | `Usn::new(i64)`, `Fid::new(u64)`, or `Fid::from_u128(u128)` |
| Raw integer flags | Typed flags; use `from_bits_retain()` and `bits()` at integer boundaries. |
| `PathResolvableEntry` methods returning `u64` IDs and `&OsString` names | Return `Fid` IDs and `&OsStr` names. |
| `UsnError::OtherError` | Specific structured error variants. |

Entry timestamps now use `usn_journal_rs::Filetime` instead of `SystemTime`.
`to_system_time()` and `from_system_time()` return `Option`; `TryFrom`
conversions return `UsnResult`. Conversion from `SystemTime` discards sub-100ns
precision toward the Unix epoch. Unix seconds/milliseconds truncate toward zero.
Detailed formatting uses local time, falling back to raw FILETIME for unformattable
values. Directory caching remains opt-in and must be rebuilt after topology changes.

## [0.4.1] - 2026-05-27

### Fixed
- Enforce Clippy lints for better code quality and improve LRU cache initialization
- Overflow checks in USN record header parsing with tests for truncated regions
- Handle root self-entry in cached path resolution
- Use backup semantics for `OpenFileById`
- Remove unaligned runtime buffer casts
- Fix mutable output pointers in journal query
- Fix MFT enumeration start file ID
- Close privilege token handles and path lookup handles on all paths

### Changed
- Redesign volume ownership API with safe handle management
- Simplify shared handle ownership model
- Simplify safe USN record parsing
- Add function to handle volume-relative path resolution
- Update `windows` crate to version 0.62.2

## [0.4.0] - 2025-08-02

### Added
- Enable test runs in CI workflows
- `IntoIterator` implementation for `Mft` and `&Mft`

### Fixed
- Fix a GitHub publish workflow bug
- Improve MFT iterator error handling
- Improve USN journal iterator error handling
- Make `filetime_to_systemtime` return `Result` and handle errors properly

### Changed
- Update `lru` crate to version 0.16
- Refactor test imports and mock volume handle
- Clarify iterator error handling in docs and examples
- Remove redundant and trivial unit tests

## [0.3.0] - 2025-06-04

### Added
- Pretty formatting for MFT and USN entries

### Changed
- Major refactoring of path resolution APIs for clarity and correctness
- Fix directory file-ID path caching
- Update examples

## [0.2.2] - 2025-05-16

### Changed
- Minor refactoring for the path module

## [0.2.1] - 2025-05-15

### Added
- Thin wrapper around the `USN_JOURNAL_DATA_V0` structure

### Fixed
- Fix doc test failures

### Changed
- Refactoring and test improvements

## [0.2.0] - 2025-05-15

### Changed
- Major refactoring to improve code readability and public API ergonomics
- Remove `FILE_FLAGS_AND_ATTRIBUTES` type from public APIs
- Update docs

## [0.1.1] - 2025-05-09

### Added
- Metadata for docs.rs targets in `Cargo.toml`

### Changed
- Documentation improvements

## [0.1.0] - 2025-05-09

### Added
- Initial release
- USN change journal iterator API
- MFT (Master File Table) enumeration iterator API
- Path resolution for MFT entries with LRU caching
- USN reason bitfield to human-readable string conversion
- Support for Windows NTFS and ReFS volumes
- CI workflow

[Unreleased]: https://github.com/wangfu91/usn-journal-rs/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/wangfu91/usn-journal-rs/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/wangfu91/usn-journal-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/wangfu91/usn-journal-rs/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/wangfu91/usn-journal-rs/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/wangfu91/usn-journal-rs/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/wangfu91/usn-journal-rs/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/wangfu91/usn-journal-rs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/wangfu91/usn-journal-rs/releases/tag/v0.1.0
