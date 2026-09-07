# Changelog

## [Unreleased] — 0.5.0

API refinements for Windows USN journals, FSCTL enumeration, and live paths.
This release is not yet published. The 0.4.1 fixes below are retained.

### Added

- Strong `Usn`, `Fid`, `FileAttributes`, `UsnReason`, and `UsnSourceInfo` types,
  including 128-bit file IDs for V3/ReFS records.
- `Filetime` at the crate root, with checked `SystemTime` conversions.
- `JournalIterOptions` and `MftIterOptions` builders, typed `UsnRecordVersion`,
  reusable iterator buffers, and a common `prelude`.
- `Volume::journal()`, `Volume::mft()`, and `Volume::path_resolver()` accessors.
- `UsnJournal::query_or_create()` for explicit create-on-demand behavior.
- Entry formatting, flag predicates, journal/path benchmarks, and regression
  tests for parsing, handle lifetimes, and path resolution.

### Changed

- Fallible iteration uses `try_iter()` and `try_iter_with_options()`.
- `query()` only queries and returns `UsnError::JournalNotActive` when absent.
  Iterator creation retains create-on-demand behavior.
- `PathResolver::new()` keeps live resolution uncached; `.with_directory_cache(n)`
  opts into caching for stable trees. `resolve_path()` accepts `&self`.
- Volume fields are private; use `drive_letter()` and `mount_point()`.
  `Volume::clone()` and iterators share ownership of the Windows handle.
- Entry timestamps use `Filetime`; identifiers and flag fields use strong types.
- `JournalIterOptionsBuilder::timeout()` accepts `Duration` in whole seconds.
- Error variants use descriptive names and structured parser diagnostics.
- Journal, MFT, and path implementations live in module directories.

### Migration from 0.4.1

| 0.4.1 API | 0.5.0 API |
| --- | --- |
| `journal.iter()` | `journal.try_iter()` |
| `mft.iter()` | `mft.try_iter()`; `IntoIterator` remains supported |
| `iter_with_options(options)` | `try_iter_with_options(options)` |
| `journal::EnumOptions` | `journal::JournalIterOptions::builder()` |
| `mft::EnumOptions` | `mft::MftIterOptions::builder()` |
| `journal.query(false)` | `journal.query()` |
| `journal.query(true)` | `journal.query_or_create()` |
| `PathResolver::new_with_cache(&volume)` | `PathResolver::new(&volume).with_directory_cache(4096)` |
| `PathResolver::new(&volume)` without cache | `PathResolver::new(&volume)` |
| `volume.drive_letter` / `volume.mount_point` | `volume.drive_letter()` / `volume.mount_point()` |
| Integer USNs and file IDs | `Usn::new(value)` and `Fid::new(value)` |
| `UsnError::PermissionError` | `UsnError::NotElevated` |
| `UsnError::WinApiError` | `UsnError::WinApi` |
| `UsnError::IoError` | `UsnError::Io` |
| `UsnError::InvalidMountPointError` | `UsnError::InvalidMountPoint` |
| `UsnError::OtherError` | Specific structured error variants |

Use `use usn_journal_rs::Filetime;` for timestamps. `to_system_time()` returns
`Option<SystemTime>`; `from_system_time()` returns `Option<Filetime>`.
Use `Filetime::try_from(system_time)` for `UsnResult<Filetime>`.
Use `Display` for entries and flags instead of the removed `pretty_format()`
and `get_reason_string()` helpers. Journal defaults live in `journal`.

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
