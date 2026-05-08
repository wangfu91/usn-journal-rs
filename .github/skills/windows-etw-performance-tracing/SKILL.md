---
name: windows-etw-performance-tracing
description: "Trace Windows Rust performance with WPR, WPA, and xperf. Use for ETW capture, disk and file I/O analysis, scheduler investigation, queue-depth and saturation checks, background-I/O diagnosis, symbol setup, and deciding whether a benchmark is CPU-bound or storage-bound. Use instead of dotnet-trace for native Rust workloads."
---

# Windows ETW Performance Tracing

Use this skill when flamegraphs stop being enough and you need to understand **Windows-native execution and I/O behavior**.

## When to Use

- Benchmark results are noisy and may be storage-bound
- Flamegraph shows `ReadFile`, `SetFilePointerEx`, seek/refill, or similar low-level I/O calls
- You need to understand disk saturation, background writes, or scheduler behavior
- You need evidence for whether to optimize parsing code or I/O locality

## Preferred Tools

- **WPR + WPA** first
- **xperf** for quick summaries or custom stackwalk capture
- **dotnet-trace** only for .NET processes, not native Rust binaries

## Procedure

1. Capture ETW with WPR using the workload-matching target.
2. Start with `GeneralProfile`, `DiskIO`, and `FileIO`.
3. Analyze the resulting `.etl` with:
   - WPA for interactive investigation
   - `xperf` summaries for quick CLI review
4. Decide whether the next work should target:
   - CPU hotspots
   - I/O locality
   - queueing / scheduling
   - benchmark environment noise

## Important Notices

- Native Rust workloads should use **WPR/WPA/xperf**, not `dotnet-trace`.
- A benchmark can regress because the disk is saturated even when flamegraph hotspots look like parser code.
- Heavy concurrent `System` writes or indexing activity can invalidate conclusions from a single benchmark run.
- Some Windows Performance Toolkit builds do **not** expose every intuitive `xperf -a <action>` summary; for example, `fileio` may not exist even though `FileIO` tracing is enabled. Use WPA and actions like `diskio`, `cpudisk`, `process`, and `filename` instead.

## References

- [ETW workflow and interpretation](./references/etw-workflow.md)

