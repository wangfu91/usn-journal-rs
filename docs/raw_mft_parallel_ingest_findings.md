# Raw MFT Parallel Ingest Findings

## Scope

This note captures the validated performance work around the parallel raw-MFT ingest path and the current measurement state for drive `C:`.

## What was kept

The current diff keeps these meaningful changes:

- Migrated `benches\raw_mft_ingest.rs` from Divan to Criterion for more stable measurements and baseline comparisons.
- Added `src\raw_mft\ingest_support.rs` so the benchmark and profiling target share the exact same ingest workload.
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
- `src\raw_mft\ingest_support.rs`
- `examples\raw_mft_parallel_ingest_profile.rs`

copied in, while leaving `src\raw_mft\*.rs` at `HEAD`.

## Benchmark results

### `C:` drive worker and scheduling sweep (May 2026)

The earlier `2 workers is best` conclusion was discarded and re-measured with
the current Criterion harness on a large `C:` NTFS volume. After that, the
benchmark defaults were corrected so `skip_unused(true)` is used consistently in
both chunk planning and scanning. That initially regressed badly because chunk
planning split at every used-record run. The planner was then changed again so
it keeps dense logical chunk bands and only drops bands that are fully unused.
The results below are from that optimized planner.

Current measured workload shape printed by `benches\raw_mft_ingest.rs`:

- drive: `C`
- addressable records: `3,059,968`
- planned chunks: about `180` on the measured live volume
- chunk size: `16,384` logical records
- main buffer: `512 KiB`
- attribute buffer: `16 KiB`
- start record: `24`
- end record: `full`

The current planner still honors `skip_unused(true)`, but it only omits chunk
bands that are completely unused. Bands that contain any used record stay dense,
so worker tasks remain coarse enough for buffered volume reads and channel/
drain overhead stays low.

Representative commands:

```powershell
$env:USN_RAW_MFT_BENCH_DRIVE='C'
$env:USN_RAW_MFT_BENCH_SCHEDULING='dynamic'
$env:USN_RAW_MFT_BENCH_WORKERS_LIST='1,2,4,6,8,10,11,12'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10

$env:USN_RAW_MFT_BENCH_DRIVE='C'
$env:USN_RAW_MFT_BENCH_WORKERS='10'
$env:USN_RAW_MFT_BENCH_SCHEDULING_LIST='dynamic,contiguous'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10

$env:USN_RAW_MFT_BENCH_PRINT_SUMMARY='1'
$env:USN_RAW_MFT_BENCH_SUMMARY_RUNS='3'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10
```

Observed medians from the optimized planner:

| Mode | Workers | Median time |
| --- | ---: | ---: |
| Dynamic | 1 | ~8.53 s |
| Dynamic | 2 | ~4.86 s |
| Dynamic | 4 | ~3.24 s |
| Dynamic | 6 | ~2.81 s |
| Dynamic | 8 | ~2.64 s |
| Dynamic | 10 | ~2.58 s |
| Dynamic | 11 | ~2.62 s |
| Dynamic | 12 | ~2.66 s |
| Contiguous | 10 | ~3.74 s |

Conclusion for this workload:

- the fastest region is now a **plateau around 8..=11 workers**, with the best point at `10`
- the best measured point was **10 workers + dynamic scheduling**
- `dynamic` was consistently and materially faster than `contiguous`

The benchmark harness now caps its automatic worker default at `10` and can also
print an opt-in one-shot summary table (`USN_RAW_MFT_BENCH_PRINT_SUMMARY=1`) so
future worker/scheduling sweeps do not require manually copying every median out
of the Criterion log.

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

## Why the `C:` results make sense in the current code

This section is intentionally limited to what the current code path can explain
directly, plus clearly marked inferences supported by the measurements.

### What each worker actually does

The current parallel path is not a producer/consumer pipeline. Each worker does
its own I/O and parsing end to end:

1. `RawMft::parallel()` builds a `RawMftParallelScan` (`src\raw_mft\parallel\scan.rs`).
2. `run_parallel_ingest()` calls `.fold_chunks(...)` with a worker count and scheduling mode (`src\raw_mft\ingest_support.rs`).
3. `for_each_folded_chunk()` forwards that work into `run_parallel_chunks_in_order()` (`src\raw_mft\parallel\chunks.rs`, `src\raw_mft\parallel\executor.rs`).
4. Each worker thread:
   - reopens the volume (`open_parallel_volume`)
   - builds **two** `VolumeReader`s (`buffered_readers_for_options`)
   - reads raw FILE records
   - parses them
   - optionally enriches from `$ATTRIBUTE_LIST` extension records
   - sends one folded chunk result back over a channel

That means increasing worker count increases both:

- useful parallelism
- and the number of independent raw-volume readers / seek streams / buffers

### Why performance improves sharply up to about 8-10 workers

The code gives a straightforward reason for the initial speedup:

- chunk execution is embarrassingly parallel at the API level
- each chunk is processed independently on a worker thread
- the main thread only drains results in order afterward

On the current `C:` workload there are about `180` planned chunks. That is
still plenty of parallel work for a moderate worker count, so scaling from
`1 -> 2 -> 4 -> 6 -> 8 -> 10` reduces how much chunk work each worker owns.

At `10` workers, the average chunk budget is still about `18` chunks per
worker, so there is enough work to keep workers busy while chunk overhead stays
amortized. The plateau is caused by the trade-off between useful parallelism and
the overhead/contention created by more workers.

### Why the optimum stops around 10 instead of scaling to 32 workers

This part is partly code fact and partly measurement-backed inference.

Facts from the code:

- every worker reopens the raw volume handle (`open_parallel_volume`)
- every worker allocates two readers (`buffered_readers_for_options`)
- every reader performs its own `SetFilePointerEx` / `ReadFile` flow (`VolumeReader::raw_seek`, `VolumeReader::refill`)
- `$ATTRIBUTE_LIST` enrichment can trigger extra extension-record loads on the per-worker `attr_reader` (`enrich_batch_from_attr_list`)

So more workers do **not** only mean more CPU. They also mean:

- more concurrent raw-volume reads
- more independent seek positions
- more buffer churn
- more extension-record lookup traffic

Inference supported by the measurements:

- by roughly `8..=10` workers, the code has extracted most of the useful scan
  parallelism available from this workload
- beyond that point, extra workers mainly add contention on the same volume and
  more competing read/seek streams, so wall-clock time stops improving and then regresses

This matches the current measured curve too: improvement is strong through the
low worker counts, then the curve flattens around `8-11`, and `10` comes out as
the best measured point.

### Why chunk cost is uneven in this benchmark

`dynamic` scheduling would not matter much if every chunk had the same cost.
The current benchmark path does **not** create equal-cost chunks.

Even after switching back to dense logical chunk bands, chunk cost still is not
uniform because the amount of *useful* work inside each band differs:

- how many valid base FILE records they contain
- how many extension records get skipped by `skip_extension_records(true)`
- how often `$ATTRIBUTE_LIST` enrichment fires
- how much string/link materialization each surviving base record needs

So one chunk may contain:

- a sparse logical band with few surviving base records
- little or no `$ATTRIBUTE_LIST` enrichment

while another chunk may contain:

- a denser logical band with many surviving base records
- more expensive FILE record parsing
- more extension-record loads from `$ATTRIBUTE_LIST`
- more string / link materialization

So even with the corrected defaults, similarly sized chunks still do not have
the same elapsed cost.

### Why `dynamic` beats `contiguous` in this code

The relevant code is in `run_parallel_chunks_in_order()`:

- `Dynamic` => each worker repeatedly grabs the next remaining chunk index from a shared atomic cursor
- `Contiguous` => each worker gets one preassigned band from `contiguous_worker_range()`

In other words:

- **dynamic** balances by *observed completion rate*
- **contiguous** balances only by *planned chunk count*

Because chunk cost varies, contiguous scheduling is vulnerable to a slow worker
ending up with a disproportionately expensive band. The whole scan then waits
for that worker's final chunks, even if other workers became idle earlier.

Dynamic scheduling reduces that tail risk:

- workers that finish cheap chunks early immediately steal the next remaining chunk
- expensive chunks get spread across whichever workers are free at the time
- total wall time is therefore closer to the cost of the busiest **set of chunks**
  rather than the cost of the unluckiest preassigned worker band

The ordered drain does not remove this benefit. Results are yielded in chunk
order, but chunk execution itself still finishes sooner when no worker is stuck
holding a heavy contiguous band.

### Why the gap between `dynamic` and `contiguous` grows at higher worker counts

This also follows naturally from the code:

- with more workers, each contiguous band contains fewer chunks
- fewer chunks per band means worse averaging of chunk-cost variance
- one expensive chunk therefore distorts a worker's entire assigned band more severely

With about `180` planned chunks, contiguous scheduling is no longer harmed by an
explosion of tiny tasks, but `dynamic` still wins because the executor sees a
stream of uneven chunk costs. Dynamic workers keep consuming the next remaining
chunk, while contiguous workers stay stuck inside one preassigned band
regardless of how expensive that band becomes.

That matches the current result at `10` workers very well:

- `dynamic`: about `3.16 s`
- `contiguous`: about `4.36 s`

So the corrected benchmark did not invalidate the scheduling conclusion.
It only changed the workload shape enough to move the worker optimum.

### What this code does *not* prove yet

The code strongly explains the *shape* of the result, but it does not by itself
prove the exact hardware bottleneck. For that, ETW / WPA or xperf would still be
the right tools.

In particular, the current source alone cannot tell us the exact split between:

- disk seek / queueing cost
- kernel-side raw-volume caching behavior
- per-worker CPU parsing cost
- memory/cache pressure from extra workers

What it *does* show is that the current executor combines I/O and parsing per
worker, uses independent volume readers per worker, and schedules logical chunks
whose true cost is heterogeneous. Given those facts, the measured `9-11`
plateau and `dynamic > contiguous` result are fully unsurprising.

## Best next directions

1. Improve **I/O locality** for parallel ingest by assigning workers more contiguous extent ranges instead of relying only on the current chunk scheduling.
2. Re-run benchmarks in a **quieter disk environment** so Criterion is measuring the ingest path instead of unrelated background writes.
3. If more evidence is needed before changing scheduling, capture a **stackwalk-enabled xperf/WPA trace** focused on disk reads, seeks, and queue depth for the ingest process.
4. Treat further string/link micro-optimizations as lower priority until the I/O-locality question is answered.
