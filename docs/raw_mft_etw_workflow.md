# Raw MFT ETW workflow

This note turns the current raw-MFT ETW tracing practice into a repeatable workflow for `usn-journal-rs` on Windows.

## What this is for

Use this workflow when:

- Criterion or one-shot timing is noisy
- flamegraph points at `ReadFile`, seek/refill, or raw-volume I/O
- you need evidence for whether the next optimization should target parsing, scheduling, or I/O locality

The exact-match target is:

- `examples\raw_mft_parallel_ingest_profile.rs`

The helper scripts are:

- `scripts\capture-raw-mft-etw.ps1`
- `scripts\summarize-raw-mft-etw.ps1`

## Prerequisites

- Elevated PowerShell session
- Windows Performance Toolkit installed and on `PATH`
  - `wpr`
  - `xperf`
- Rust toolchain and `cargo`

## Recommended baseline shape

Unless you are testing a different hypothesis, start with the same benchmark shape used in the recent raw-MFT work:

- `workers=11`
- `chunk_records=2048`
- `main_buffer=262144`
- `attr_buffer=16384`
- `summary-light`
- `offset-sorted attr-list loads`

## Capture one scheduling mode

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile `
  -SummarizeWithXperf
```

This script:

1. builds `raw_mft_parallel_ingest_profile` in release mode
2. starts WPR with `GeneralProfile + FileIO + DiskIO`
3. runs the exact-match executable directly
4. stops WPR into a timestamped `.etl`
5. saves stdout / stderr / capture metadata beside the trace
6. optionally runs `xperf` text summaries

Output goes under:

- `tmp\raw_mft_parallel_ingest_validation\etw\...`

## Capture two scheduling modes back to back

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic,contiguous `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile `
  -SummarizeWithXperf
```

## Capture the experimental physical-order scheduling mode

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic-physical-order `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile
```

## Capture the experimental cost-aware banded scheduling mode

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic-cost-banded `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile
```

## Capture the experimental observed-adaptive scheduling mode

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic-observed-adaptive `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile `
  -SummarizeWithXperf
```

## Capture an experimental deferred attr-list run

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile `
  -DeferredAttrList `
  -DeferredAttrListWindowRecords 256
```

## Summarize an existing `.etl`

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\summarize-raw-mft-etw.ps1 `
  -EtlPath .\tmp\raw_mft_parallel_ingest_validation\etw\raw_mft_parallel_ingest_dynamic_YYYYMMDD-HHMMSS\raw_mft_parallel_ingest_dynamic_YYYYMMDD-HHMMSS.etl
```

This writes text summaries such as:

- `diskio.txt`
- `diskio-detail.txt`
- `cpudisk.txt`
- `process.txt`
- `filename.txt`
- `README.md`

## How to interpret the outputs

Start with these questions:

1. Is the ingest process saturating the disk?
2. Is queue depth staying high enough during the read burst?
3. Are background `System` writes or other processes distorting the run?
4. Does a candidate change improve service time or just rearrange reads without wall-clock benefit?

### If WPA is available

Open the `.etl` and inspect:

- Disk Usage
- CPU Usage (Precise)
- File I/O
- process timelines around the ingest executable and `System`

### If you stay in CLI first

Check:

- `diskio-detail.txt` for read counts, service time, and queue depth style signals
- `process.txt` for process activity windows
- `cpudisk.txt` for CPU vs disk overlap
- `filename.txt` for dominant path activity

## Current practical guidance

Based on the recent experiments in `docs\raw_mft_parallel_ingest_findings.md`:

- plain `dynamic` remains the retained default
- deferred attr-list loading and physical-order dynamic are useful experiments, but neither has yet produced a strong enough end-to-end win to replace the default
- ETW should now be used mainly to decide whether the next experiment needs:
  - stronger cost-aware scheduling
  - better chunk grouping / banding
  - or simply a quieter benchmark environment



