# Raw MFT Parallel Ingest Findings

## Scope

This note captures the validated performance work around the parallel raw-MFT ingest path and the current measurement state for drive `C:`.

## What was kept

The current diff keeps these meaningful changes:

- Migrated `benches\raw_mft_ingest.rs` from Divan to Criterion for more stable measurements and baseline comparisons.
- Added `benches\support\raw_mft_ingest_shared.rs` so the benchmark and profiling target share the exact same ingest workload.
- Added `examples\raw_mft_parallel_ingest_profile.rs` as the exact-match profiling target for flamegraph and ETW runs.
- Kept the lean batch parsing path in `src\raw_mft\batch.rs` and `src\raw_mft\mod.rs`, so folded chunk consumers avoid rebuilding the full `RawMftEntry` shape when they only need the reduced batch form.

## Validation method

To validate that the parser-side optimizations in the current diff are still worth keeping, the current Criterion harness was copied into a temporary detached `HEAD` worktree and benchmarked there. That isolates the parser changes from the benchmark-harness changes.

Commands used:

```powershell
cargo bench --bench raw_mft_ingest -- --sample-size 10
```

`HEAD` was benchmarked in a detached validation worktree with the current:

- `Cargo.toml`
- `benches\raw_mft_ingest.rs`
- `benches\support\raw_mft_ingest_shared.rs`
- `examples\raw_mft_parallel_ingest_profile.rs`

copied in, while leaving `src\raw_mft\*.rs` at `HEAD`.

## Benchmark results

### Current worktree vs detached `HEAD` with the same harness

| Revision | Result |
| --- | --- |
| Detached `HEAD` + current harness | `time: [3.4421 s 3.4763 s 3.5208 s]` |
| Current worktree | `time: [3.4310 s 3.4428 s 3.4544 s]` |

Conclusion: the current parser-side diff is still a real win and was **kept**.

### Current worktree vs saved Criterion baseline

The saved `round2-start` baseline in the current worktree was:

| Baseline | Result |
| --- | --- |
| `round2-start` | `time: [3.6494 s 3.6835 s 3.7254 s]` |
| Current worktree rerun | `time: [3.4310 s 3.4428 s 3.4544 s]` |

Criterion also reported:

```text
change: [-10.182% -9.1767% -8.3310%] (p = 0.00 < 0.05)
Performance has improved.
```

This is encouraging, but the detached-`HEAD` comparison above is the more important validation because it isolates the kept source changes from benchmark harness changes.

## Experiments that were rejected

Two aggressive follow-up experiments were tried and then reverted because they regressed:

- Replacing batch-path names with a raw UTF-16 representation.
- Adding an ingest-specialized fold path that precomputed visible parent references.

Those attempts made the benchmark slower, so they were not kept in the current diff.

## Profiling findings

### Flamegraph

The exact-match flamegraph continued to show hot costs in:

- `OsString` cloning and UTF-16 file-name materialization.
- `RawMftLink` growth and copying.
- `$ATTRIBUTE_LIST` reads and enrichment.
- Volume seek/read work in `VolumeReader`.

### ETW / Disk behavior

An ETW trace was captured with:

```powershell
wpr -start GeneralProfile -start FileIO -start DiskIO -filemode
cargo run --release --example raw_mft_parallel_ingest_profile
wpr -stop raw_mft_parallel_ingest_round2.etl
```

The important findings were:

- `Disk 0` hit roughly `93%` to `99%` utilization during the main ingest burst.
- The exact-match profile example completed in about `4.854s` during that ETW run.
- There was meaningful concurrent background disk activity during the capture, including heavy writes attributed to `System`.

Conclusion: the next bottleneck is not purely parser CPU work anymore; storage behavior and background I/O are materially affecting results.

## Best next directions

1. Improve **I/O locality** for parallel ingest by assigning workers more contiguous extent ranges instead of relying only on the current chunk scheduling.
2. Re-run benchmarks in a **quieter disk environment** so Criterion is measuring the ingest path instead of unrelated background writes.
3. If more evidence is needed before changing scheduling, capture a **stackwalk-enabled xperf/WPA trace** focused on disk reads, seeks, and queue depth for the ingest process.
4. Treat further string/link micro-optimizations as lower priority until the I/O-locality question is answered.
