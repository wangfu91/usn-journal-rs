[![Crates.io](https://img.shields.io/crates/v/usn-journal-rs.svg)](https://crates.io/crates/usn-journal-rs)
[![Docs.rs](https://docs.rs/usn-journal-rs/badge.svg)](https://docs.rs/usn-journal-rs)
[![CI](https://github.com/wangfu91/usn-journal-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/wangfu91/usn-journal-rs/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

# usn-journal-rs

Safe, ergonomic Rust bindings for the Windows NTFS/ReFS USN change journal and
Master File Table (MFT).

## Overview

**usn-journal-rs** lets you read the USN change journal, enumerate the MFT via
the Windows FSCTL APIs, and parse the raw `$MFT` file directly for rich
per-record metadata. It exposes idiomatic Rust iterators and builder-pattern
option structs over the underlying `DeviceIoControl` calls.

The crate is **Windows-only**. It targets NTFS and ReFS volumes and requires the
calling process to be running as Administrator — raw volume handles and the USN
journal IOCTLs are privilege-gated by the OS.

## Features

- Read and iterate `USN_RECORD_V2` / `USN_RECORD_V3` journal records with a configurable reason mask and start USN
- Enumerate MFT entries via the `FSCTL_ENUM_USN_DATA` API, including ReFS 128-bit file IDs
- Parse raw `$MFT` records (NTFS only) for full timestamps, real/allocated sizes, hard-link
  counts, alternate data streams, and sparse/compressed/encrypted flags
- Resolve file IDs to full paths with three strategies: syscall-only, LRU-cached,
  or an in-memory directory tree for O(1) resolution on large scans
- Lightweight `Filetime(u64)` newtype with standard-library conversions
- Strong `Usn`, `Fid`, `UsnReason`, and `FileAttributes` types throughout (`Fid` supports both 64-bit NTFS and 128-bit ReFS file IDs)
- `usn_journal_rs::prelude` for the common high-level types and bitflags

## Quick start

Add to `Cargo.toml`:

```toml
[dependencies]
usn-journal-rs = "0.5"
```

Iterate the USN change journal on drive `C:`:

```rust
use usn_journal_rs::errors::UsnError;
use usn_journal_rs::journal::{JournalIterOptions, UsnEntry, UsnJournal, USN_REASON_MASK_ALL};
use usn_journal_rs::volume::Volume;
use usn_journal_rs::{Usn, UsnReason};
use std::num::NonZeroUsize;

fn main() -> Result<(), UsnError> {
    let volume = Volume::from_drive_letter('C')?;
    let journal = UsnJournal::new(&volume);

    let opts = JournalIterOptions::builder()
        .start_usn(Usn::new(0))
        .reason_mask(UsnReason::from_bits_retain(USN_REASON_MASK_ALL))
        .only_on_close(false)
        .buffer_bytes(NonZeroUsize::new(64 * 1024).unwrap())
        .build();

    for result in journal.try_iter_with_options(opts)? {
        let entry: UsnEntry = result?;
        println!("{}", entry); // compact one-line Display
    }
    Ok(())
}
```

## Examples

| Example          | Description                                 | Run                                  |
| ---------------- | ------------------------------------------- | ------------------------------------ |
| `read_journal`   | Iterate all USN journal records on a volume | `cargo run --example read_journal`   |
| `enum_mft`       | Enumerate every MFT entry via FSCTL         | `cargo run --example enum_mft`       |
| `raw_mft`        | Parse raw `$MFT` records with full metadata | `cargo run --example raw_mft`        |
| `change_monitor` | Watch for live filesystem changes via USN   | `cargo run --example change_monitor` |
| `pretty_print`   | Multi-line formatted output for USN entries | `cargo run --example pretty_print`   |

All examples require Administrator privileges.

## Performance notes

Benchmarks are run with [Divan](https://github.com/nvzqz/divan) on a 200 k-record NTFS volume.

- **Raw `$MFT` iteration** — ~6× faster than 0.4.x (262 ms vs 1.64 s). Achieved via
  zero-copy fixup parsing (`VolumeReader::borrow_at`) and elimination of per-record memcpy.
- **Default syscall path resolution** — `PathResolver::new(&volume)` now includes an
  LRU directory cache out of the box, so USN/MFT scans avoid the old uncached-by-default
  behavior unless you explicitly opt out with `.without_lru_cache()`.
- **In-memory directory-tree path resolution** — ~40× faster than the syscall-based resolver
  for full-volume scans (<500 ms vs ~21 s). Use `PathResolver::new(v).with_in_memory_tree(&raw_mft)?`.
- **Buffer size** — tune with `RawMftScanOptions::builder().buffer_bytes(NonZeroUsize::new(256 * 1024).unwrap()).build()`.

For the newer raw-`$MFT` ingest throughput work, use the Criterion harness in
`benches/raw_mft_ingest.rs` instead of the ad-hoc profiling example when you
need statistically useful worker-count or scheduling comparisons.

Run benchmarks:

```powershell
cargo bench --bench raw_mft
cargo bench --bench journal
cargo bench --bench path_resolver
cargo bench --bench raw_mft_ingest
```

Set `USN_TEST_DRIVE=D` to target a different volume (default: `C`).

The raw-`$MFT` ingest harness also understands a few environment variables:

- `USN_RAW_MFT_BENCH_DRIVE=C` — choose the target volume
- `USN_RAW_MFT_BENCH_WORKERS=10` — set one fixed worker count
- `USN_RAW_MFT_BENCH_WORKERS_LIST=1,2,4,8,11` — sweep worker counts in one run
- `USN_RAW_MFT_BENCH_SCHEDULING=dynamic` — choose the executor policy for the baseline run
- `USN_RAW_MFT_BENCH_SCHEDULING_LIST=dynamic,contiguous` — compare both policies side by side
- `USN_RAW_MFT_BENCH_PRINT_SUMMARY=1` — print an extra one-shot summary table before Criterion runs
- `USN_RAW_MFT_BENCH_SUMMARY_RUNS=3` — use a median of 3 one-shot runs per summary row

### Raw `$MFT` ingest benchmark notes

Recent Criterion runs on a large `C:` NTFS volume used the current default
ingest benchmark shape, where both chunk planning and scanning keep
`skip_unused(true)`, but chunk planning still uses dense logical bands and only
drops fully unused bands:

- ~3,059,968 addressable records
- 16,384 records per chunk (~180 planned chunks on the measured live volume)
- 512 KiB main buffer / 16 KiB attribute buffer

Observed results for that workload:

- **Dynamic scheduling** clearly beat **contiguous scheduling**
- The fastest point stayed around **10 workers**
- The best measured point was **10 workers + dynamic scheduling**

Representative medians from that run:

| Mode | Workers | Median time |
| ---- | ------: | ----------: |
| Dynamic | 2 | ~4.86 s |
| Dynamic | 4 | ~3.24 s |
| Dynamic | 6 | ~2.81 s |
| Dynamic | 10 | ~2.58 s |
| Dynamic | 11 | ~2.62 s |
| Contiguous | 10 | ~3.74 s |

That is why the ingest benchmark defaults now cap the automatic worker count at
10 instead of following all available logical CPUs. Always re-measure on the
actual target volume before treating a result as universal: filesystem churn and
different used-record fragmentation can shift both the planned chunk count and
the optimum worker count.

For the longer write-up, including the `C:`-drive sweep data and a code-based
explanation of why `dynamic` scheduling wins, see
[`docs/raw_mft_parallel_ingest_findings.md`](docs/raw_mft_parallel_ingest_findings.md).

## Privileges

All APIs that open a volume (`Volume::from_drive_letter`, `Volume::from_mount_point`) require
the process to run as **Administrator**. On non-elevated processes the crate returns
`UsnError::NotElevated` before attempting any system call.

## Filesystem support

| Feature                 | NTFS | ReFS                                          |
| ----------------------- | ---- | --------------------------------------------- |
| USN journal             | ✅    | ✅                                             |
| MFT enumeration (`Mft`) | ✅    | ✅                                             |
| Raw `$MFT` (`RawMft`)   | ✅    | ❌ — returns `UsnError::UnsupportedFilesystem` |

On ReFS, journal and `Mft` entries may expose 128-bit file IDs via
`Fid::is_extended()`, `Fid::as_u128()`, and `Fid::as_bytes()`.

## Migrating from 0.4.x

See [CHANGELOG.md](CHANGELOG.md) for a full list of breaking changes and before/after
migration snippets.

## License

MIT License. See [LICENSE](LICENSE) for details.
