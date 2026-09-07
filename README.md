[![Crates.io](https://img.shields.io/crates/v/usn-journal-rs.svg)](https://crates.io/crates/usn-journal-rs)
[![Docs.rs](https://docs.rs/usn-journal-rs/badge.svg)](https://docs.rs/usn-journal-rs)
[![CI](https://github.com/wangfu91/usn-journal-rs/actions/workflows/rust.yml/badge.svg)](https://github.com/wangfu91/usn-journal-rs/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

# usn-journal-rs

Safe, ergonomic Rust APIs for the Windows NTFS/ReFS USN change journal and FSCTL MFT enumeration.

Raw NTFS `$MFT` reading now lives in the independent
[ntfs-mft](https://github.com/wangfu91/ntfs-mft) repository.

## Overview

**usn-journal-rs** reads the USN change journal and enumerates MFT entries via
Windows FSCTL APIs. It exposes Rust iterators and builder options over
`DeviceIoControl`, plus live path resolution. These operations require Windows
and Administrator privileges.

## Features

- Read and iterate `USN_RECORD_V2` / `USN_RECORD_V3` journal records with a configurable reason mask and start USN
- Enumerate MFT entries via the `FSCTL_ENUM_USN_DATA` API, including ReFS 128-bit file IDs
- Resolve current file IDs to full paths with optional LRU directory caching
- Lightweight `Filetime(u64)` newtype with standard-library conversions
- Strong `Usn`, `Fid`, `UsnReason`, and `FileAttributes` types throughout (`Fid` supports both 64-bit NTFS and 128-bit ReFS file IDs)
- `usn_journal_rs::prelude` for the common high-level types and bitflags

## Quick start

The examples below target the unreleased 0.5.0 API on `api-refine`.
Use the Git dependency until that version is published:

```toml
[dependencies]
usn-journal-rs = { git = "https://github.com/wangfu91/usn-journal-rs", branch = "api-refine" }
```

Iterate the USN change journal on drive `C:`:

```rust
use usn_journal_rs::errors::UsnError;
use usn_journal_rs::journal::{JournalIterOptions, UsnEntry, UsnJournal};
use usn_journal_rs::volume::Volume;
use usn_journal_rs::{Usn, UsnReason};
use std::num::NonZeroUsize;

fn main() -> Result<(), UsnError> {
    let volume = Volume::from_drive_letter('C')?;
    let journal = UsnJournal::new(&volume);

    let opts = JournalIterOptions::builder()
        .start_usn(Usn::ZERO)
        .reason_mask(UsnReason::ALL)
        .only_on_close(false)
        .buffer_bytes(NonZeroUsize::new(64 * 1024).unwrap())
        .build()?;

    for result in journal.try_iter_with_options(opts)? {
        let entry: UsnEntry = result?;
        println!("{}", entry); // compact one-line Display
    }
    Ok(())
}
```

### Watch for live changes

`Volume` exposes convenience accessors — `journal()`, `mft()`, and
`path_resolver()` — so you rarely need to import the reader types directly. To
follow the journal tail and resolve each change to a full path:

```rust
use usn_journal_rs::errors::UsnError;
use usn_journal_rs::journal::JournalIterOptions;
use usn_journal_rs::volume::Volume;

fn main() -> Result<(), UsnError> {
    let volume = Volume::from_drive_letter('C')?;
    let journal = volume.journal();
    let resolver = volume.path_resolver(); // resolve_path takes &self

    // Start at the current tail, then block waiting for new records.
    let tail = journal.query_or_create()?.next_usn;
    let opts = JournalIterOptions::builder()
        .start_usn(tail)
        .wait_for_more(true)
        .build()?;

    for result in journal.try_iter_with_options(opts)? {
        let entry = result?;
        // Named predicates make change classification read naturally:
        let kind = if entry.reason.is_file_create() {
            "created"
        } else if entry.reason.is_file_delete() {
            "deleted"
        } else if entry.reason.is_rename() {
            "renamed"
        } else {
            "modified"
        };
        match resolver.resolve_path(&entry) {
            Some(path) => println!("{kind}: {}", path.display()),
            None => println!("{kind}: {}", entry.file_name.to_string_lossy()),
        }
    }
    Ok(())
}
```

## Examples

| Example                   | Description                                                      | Run                                            |
| ------------------------- | ---------------------------------------------------------------- | ---------------------------------------------- |
| `read_journal`            | Iterate all USN journal records on a volume                      | `cargo run --features windows-examples --example read_journal` |
| `enum_mft`                | Enumerate every MFT entry via FSCTL                              | `cargo run --features windows-examples --example enum_mft` |
| `change_monitor`          | Watch for live filesystem changes via USN                        | `cargo run --features windows-examples --example change_monitor` |
| `journal_pretty_print`    | Multi-line formatted output for USN entries                      | `cargo run --features windows-examples --example journal_pretty_print` |

The examples require Windows and Administrator privileges. `read_journal`,
`enum_mft`, and `change_monitor` accept a drive letter such as `C` or `C:`
as the first argument (default `C`).

## Development

Run `cargo build`, `cargo test --all-features`, and
`cargo clippy --all-targets --all-features -- -D warnings` on Windows.
Device-dependent tests skip when privileges or a suitable volume are unavailable.
`cargo package` verifies the publishable crate without publishing it.

Cloned volumes and iterators share handle ownership; an iterator can outlive
the volume value that created it.

## Benchmarks

```text
cargo bench --features windows-examples --bench journal
cargo bench --features windows-examples --bench path_resolver
```

## Privileges and filesystem support

Journal, FSCTL enumeration, and live path operations require Windows and an
Administrator process. Journal and enumeration APIs support NTFS/ReFS; ReFS
entries may use extended file IDs.

On ReFS, journal and `Mft` entries may expose 128-bit file IDs via
`Fid::is_extended()`, `Fid::as_u128()`, and `Fid::as_bytes()`.

## API behavior

Journal `iter()?` (also available as `try_iter()?`) reads available records by
default. Set `wait_for_more(true)` to wait for changes. A positive fractional
timeout rounds up to whole seconds; zero means an indefinite kernel wait.
Options builders validate their inputs and return `Result` from `build()`.
MFT `iter()` constructs an iterator directly; each item is still a `Result`.

Use `error.is_permission_denied()` to recognize permission failures, including
underlying Win32 errors. Use `resolver.try_resolve_path(&entry)` when you need
the lookup error; `resolve_path(&entry)` returns `None` on failure.

`entry.pretty_format(resolved_path)` produces detailed output; `Display`
produces a compact summary. Custom `PathResolvableEntry` implementations remain
supported. `new_with_cache()` and `path_resolver_with_cache()` retain their
4096-entry default; enable caching only for stable trees.

## Migrating from 0.4.x

See [CHANGELOG.md](CHANGELOG.md) for the 0.4.1 migration table and release history.

## License

MIT License. See [LICENSE](LICENSE) for details.
