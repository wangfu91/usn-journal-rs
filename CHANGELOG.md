# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
