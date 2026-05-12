# `raw_mft`

This directory contains the internal implementation of the raw NTFS `$MFT` reader.

The code here is organized by responsibility:

- `layout/` — on-disk NTFS structures and low-level parsers
- `reader.rs` — reads raw FILE records from the volume
- `entry_build/` — builds higher-level `RawMftEntry` values
- `bootstrap/` — discovers the `$MFT` streams and prepares the scan
- `serial/` and `parallel/` — scanning engines
- `chunk_plan.rs` — chunk planning for parallel scans
- `io.rs` — sector-aligned volume I/O helpers
- `options.rs` — internal scan and entry options
- `tests.rs` — module-level tests

## Raw MFT read flow

```mermaid
flowchart TD
    A[Read NTFS Boot Sector<br/>`layout/boot.rs`] --> B[Parse volume geometry<br/>- bytes per sector<br/>- cluster size<br/>- MFT start LCN]
    B --> C[Read the `$MFT::$DATA` runlist]
    C --> D[Decode data runs<br/>`layout/data_run.rs`]
    D --> E[Build extent map<br/>`layout/extent.rs`]
    E --> F[Resolve record number to absolute volume offset<br/>through fragmented extents when needed]
    F --> G[Read raw FILE record bytes<br/>`reader.rs` / `VolumeReader`]
    G --> H[Validate FILE record header<br/>`layout/record.rs`]
    H --> I[Apply USA fixup<br/>`layout/usa_fixup.rs`]
    I --> J[Obtain a valid FILE record buffer]
    J --> K[Parse attributes, names, and streams<br/>`layout/attribute/*` + `entry_build/*`]
```

## What `extent.rs` does

```mermaid
flowchart TD
    A[Decoded `DataRun` list] --> B[`ExtentMap::from_runs()`]
    B --> C[Store VCN-to-LCN segments]
    C --> D[`record_offset(record_number)`]
    D --> E[Convert record number to VCN]
    E --> F[Find matching extent segment]
    F --> G{Segment is sparse?}
    G -- Yes --> H[Return `None`]
    G -- No --> I[Compute absolute byte offset]
    I --> J[Return disk offset]

    D --> K[`record_offset_with_cursor()`]
    K --> L[Reuse cached segment index]
    L --> F
```

`extent.rs` turns the `$MFT` runlist into a lookup structure for:

> `record number -> absolute disk byte offset`

It is needed because the `$MFT` is often **not stored contiguously** on disk. A simple formula like `mft_start + record_number * file_record_size` only works when the MFT is effectively one contiguous extent. When a caller wants to read record `N`, the code first needs to determine which extent segment contains that record and where that segment maps on the volume.

## Short summary

```text
boot sector
  -> MFT runlist
  -> data runs
  -> extent map
  -> record offset
  -> read FILE record
  -> USA fixup
  -> parse attributes
```

## Notes

- This README is intentionally internal and describes the implementation structure of `raw_mft`.
- The diagrams are written in English so they can be reused in docs or design notes.

## Parallel ingest benchmarking

For worker-count and executor-policy questions, prefer the Criterion harness in
`benches/raw_mft_ingest.rs` over `examples/raw_mft_parallel_ingest_profile.rs`.
The example is still useful for profiling, but it is not a statistically sound
benchmark on its own.

Useful environment variables:

- `USN_RAW_MFT_BENCH_DRIVE=C`
- `USN_RAW_MFT_BENCH_WORKERS=10`
- `USN_RAW_MFT_BENCH_WORKERS_LIST=1,2,4,8,11`
- `USN_RAW_MFT_BENCH_SCHEDULING=dynamic`
- `USN_RAW_MFT_BENCH_SCHEDULING_LIST=dynamic,contiguous`
- `USN_RAW_MFT_BENCH_CHUNK_RECORDS=16384`
- `USN_RAW_MFT_BENCH_PRINT_SUMMARY=1`
- `USN_RAW_MFT_BENCH_SUMMARY_RUNS=3`

Example commands:

```powershell
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10

$env:USN_RAW_MFT_BENCH_DRIVE='C'
$env:USN_RAW_MFT_BENCH_WORKERS_LIST='1,2,4,8,11'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10
```

Recent Criterion runs on a large `C:` NTFS volume, using the current ingest
benchmark shape (~3.06 M addressable records, `skip_unused(true)` in both chunk
planning and scanning, but dense logical chunk bands with only fully unused
bands omitted, and about 180 planned chunks on the measured live volume),
showed:

- `dynamic` scheduling consistently beating `contiguous`
- the fastest point landing around `10` workers
- the best measured point at `10` workers with `dynamic`

Representative medians:

| Mode | Workers | Median time |
| ---- | ------: | ----------: |
| Dynamic | 2 | ~4.86 s |
| Dynamic | 4 | ~3.24 s |
| Dynamic | 6 | ~2.81 s |
| Dynamic | 10 | ~2.58 s |
| Dynamic | 11 | ~2.62 s |
| Contiguous | 10 | ~3.74 s |

Those measurements only justify the current **benchmark** default. Production
scan defaults should still be chosen from the calling workload, not copied from
one benchmark volume blindly.

For the fuller experiment log and the code-path explanation of the worker-count
and scheduling results, see
[`../../docs/raw_mft_parallel_ingest_findings.md`](../../docs/raw_mft_parallel_ingest_findings.md).

