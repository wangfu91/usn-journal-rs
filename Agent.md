# Agent Guide

## Scope

`usn-journal-rs` owns Windows USN journal APIs, FSCTL MFT enumeration and live
path resolution. Preserve the released 0.4.1 correctness fixes when refining APIs.
See README.md and CHANGELOG.md for the current API and migration guide.

## Validation

Run `cargo build`, `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`,
and `cargo package`. Windows device tests require Administrator privileges and
must skip unsuitable environments. Examples are:
`read_journal`, `enum_mft`, `change_monitor`, `journal_pretty_print`.
Benchmarks are `journal` and `path_resolver`. Use Criterion for
performance claims. `USN_TEST_DRIVE` selects the test volume, `USN_REFS_TEST_DRIVE`
selects ReFS coverage, and `BENCH_RECORD_LIMIT` bounds journal benchmarks.

## Code and conventions

- `src/journal` and `src/mft` own the Windows APIs and fallible iterators.
- `src/path` owns live file-ID resolution with optional LRU directory caching.
- `src/volume.rs` owns shared Windows volume handles and source accessors.
- `src/usn_record.rs` parses Windows USN buffers; reuse its checked parsing.
- `src/types.rs`, `src/time.rs`, and `src/errors.rs` own strong identifiers,
  Filetime, and the non-exhaustive UsnError enum.
- Keep unsafe Win32 interaction localized at existing FFI boundaries.
- Use UsnResult and concrete UsnError variants; retain strong public domain types.
- Iterators yield errors per entry instead of aborting the entire scan.
- Fid::Standard stores NTFS references; Fid::Extended supports ReFS/v3 IDs.
- PathResolver is uncached by default; opt into caching only for stable trees.
