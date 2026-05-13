# Raw MFT Parallel Ingest Findings

## Scope

This note captures the validated performance work around the parallel raw-MFT ingest path and the current measurement state for drive `C:`.

## What was kept

The current diff keeps these meaningful changes:

- Migrated `benches\raw_mft_ingest.rs` from Divan to Criterion for more stable measurements and baseline comparisons.
- Added `src\raw_mft\ingest_support.rs` so the benchmark and profiling target share the exact same ingest workload.
- Added `examples\raw_mft_parallel_ingest_profile.rs` as the exact-match profiling target for flamegraph and ETW runs.
- Added an experimental `dynamic-cost-banded` scheduler in `src\raw_mft\parallel\executor.rs`, wired through the benchmark/profile tooling so higher-level cost-aware scheduling experiments can now be measured without changing the retained default.
- Kept the lean batch parsing path in `src\raw_mft\entry_build\batch.rs`, with the batch types wired through `src\raw_mft\entry_build\mod.rs` and re-exported from `src\raw_mft\mod.rs`, so folded chunk consumers avoid rebuilding the full `RawMftEntry` shape when they only need the reduced batch form.

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

### `C:` drive chunk, worker, scheduling, and buffer sweeps (May 2026)

The earlier `2 workers is best` conclusion was discarded and re-measured with
the current Criterion harness on a large `C:` NTFS volume. After that, the
benchmark defaults were corrected so `skip_unused(true)` is used consistently in
both chunk planning and scanning. That initially regressed badly because chunk
planning split at every used-record run. The planner was then changed again so
it keeps dense logical chunk bands and only drops bands that are fully unused.
The results below are from that optimized planner.

Current measured workload shape printed by `benches\raw_mft_ingest.rs` after
retuning the benchmark defaults:

- drive: `C`
- addressable records: `3,059,968`
- planned chunks: about `1,329` on the measured live volume
- chunk size: `2,048` logical records
- main buffer: `256 KiB`
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
$env:USN_RAW_MFT_BENCH_CHUNK_RECORDS='2048'
$env:USN_RAW_MFT_BENCH_WORKERS_LIST='1,2,4,6,8,10,11,12'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10

$env:USN_RAW_MFT_BENCH_DRIVE='C'
$env:USN_RAW_MFT_BENCH_CHUNK_RECORDS='2048'
$env:USN_RAW_MFT_BENCH_WORKERS='11'
$env:USN_RAW_MFT_BENCH_SCHEDULING_LIST='dynamic,contiguous'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10

$env:USN_RAW_MFT_BENCH_DRIVE='C'
$env:USN_RAW_MFT_BENCH_CHUNK_RECORDS='2048'
$env:USN_RAW_MFT_BENCH_WORKERS='11'
$env:USN_RAW_MFT_BENCH_SCHEDULING='dynamic'
$env:USN_RAW_MFT_BENCH_BUFFER_BYTES='262144'
$env:USN_RAW_MFT_BENCH_ATTR_BUFFER_BYTES='16384'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10

$env:USN_RAW_MFT_BENCH_PRINT_SUMMARY='1'
$env:USN_RAW_MFT_BENCH_SUMMARY_RUNS='3'
cargo bench --bench raw_mft_ingest -- --sample-size 10 --warm-up-time 3 --measurement-time 10
```

Observed medians from the current chunk-size sweep (`workers=10`, `dynamic`,
default buffers at the time):

| Chunk records | Planned chunks | Median time |
| --- | ---: | ---: |
| 1,024 | 2,595 | ~2.53 s |
| 2,048 | 1,329 | ~2.49 s |
| 4,096 | 680 | ~2.51 s |
| 8,192 | 351 | ~2.56 s |

Observed medians from the follow-up worker and scheduling sweeps with
`chunk_records=2048`:

| Mode | Workers | Median time |
| --- | ---: | ---: |
| Dynamic | 8 | ~2.55 s |
| Dynamic | 10 | ~2.50 s |
| Dynamic | 11 | ~2.48 s |
| Dynamic | 12 | ~2.51 s |
| Contiguous | 11 | ~3.67 s |

Observed medians from the main-buffer sweep with `chunk_records=2048`,
`workers=11`, `dynamic`, and `attr_buffer=16 KiB`:

| Main buffer | Median time |
| --- | ---: |
| 64 KiB | ~2.51 s |
| 128 KiB | ~2.38 s |
| 256 KiB | ~2.35 s |
| 512 KiB | ~2.38 s |
| 1 MiB | ~2.41 s |
| 2 MiB | ~2.44 s |

Observed medians from the attribute-buffer sweep with `chunk_records=2048`,
`workers=11`, `dynamic`, and `main_buffer=256 KiB`:

| Attribute buffer | Median time |
| --- | ---: |
| 4 KiB | ~2.39 s |
| 8 KiB | ~2.35 s |
| 16 KiB | ~2.35 s |
| 32 KiB | ~2.41 s |
| 64 KiB | ~2.64 s |

Conclusion for this workload:

- the fastest region is now a **plateau around 10..=11 workers**
- the best measured point so far is **11 workers + dynamic scheduling + 2,048-record chunks + 256 KiB / 16 KiB buffers**
- `dynamic` was consistently and materially faster than `contiguous`
- `256 KiB` is the current best measured main-buffer default for this benchmark shape
- `16 KiB` remains a good attribute-buffer default; `8 KiB` was effectively tied, while larger buffers regressed

The benchmark harness now caps its automatic worker default at `10`, defaults to
`2,048` logical records per chunk, uses a `256 KiB` main buffer and a `16 KiB`
attribute buffer, and can also print an opt-in one-shot summary table
(`USN_RAW_MFT_BENCH_PRINT_SUMMARY=1`) so future worker/scheduling sweeps do not
require manually copying every median out of the Criterion log.

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

Another scheduling-focused experiment was tried during the May 2026 locality
work and also was **not** kept as a code change:

- Add a batched dynamic scheduler that hands out small contiguous chunk groups
  (for example 2, 4, or 8 chunks at a time) from the shared atomic cursor,
  hoping to preserve more worker-local sequential I/O than plain per-chunk
  dynamic scheduling while keeping better balance than fully contiguous bands.

Initial sweep at `11` workers, `2048` chunk records, and `256 KiB` / `16 KiB`
buffers looked mildly promising for the 2-chunk batch:

| Scheduling | Median time |
| --- | ---: |
| Dynamic | ~2.43 s |
| Dynamic batched, 2 chunks | ~2.38 s |
| Dynamic batched, 4 chunks | ~2.43 s |
| Dynamic batched, 8 chunks | ~2.51 s |
| Contiguous | ~3.36 s |

But two follow-up confirmation runs with fixed worker counts did **not** hold up
that apparent win:

| Workers | Dynamic | Dynamic batched, 2 chunks |
| ---: | ---: | ---: |
| 10 | ~2.34 s | ~2.36 s |
| 11 | ~2.35 s | ~2.36 s |

Conclusion: batching chunk claims may slightly reduce the atomic scheduling rate,
but on this workload it does not produce a stable enough locality win to beat or
replace plain dynamic scheduling. The experiment was kept as a measurement note,
not as a retained code path.

Another attr-list-locality experiment was also tried and measured with the same
Criterion harness shape (`11` workers, `2048` chunk records, `256 KiB` main
buffer, `16 KiB` attribute buffer, `summary-light`, offset-sorted extension
loads):

- Defer batch `$ATTRIBUTE_LIST` enrichment until the whole chunk has been
  scanned, then sort all referenced extension-record loads for that chunk by
  physical offset before reading and merging them back into the original batch
  entries.

This experiment materially improved the extension-load locality counters in the
exact-match profile run:

- exact-adjacent extension loads rose from about `8.6%` to about `22.8%`
- extension-load jumps `<= 1 MiB` rose from about `31.8%` to about `72.0%`
- extension-load jumps `> 64 MiB` fell from about `46.3%` to about `12.4%`

But the end-to-end benchmark result was still too small to count as a retained
win on this workload:

| Mode | Criterion result |
| --- | --- |
| Legacy per-record attr-list enrichment | `time: [1.3547 s 1.3633 s 1.3736 s]` |
| Deferred chunk attr-list enrichment | `time: [1.3474 s 1.3517 s 1.3559 s]` |

Criterion reported:

```text
change: [−1.6565% −0.8521% −0.1398%] (p = 0.04 < 0.05)
Change within noise threshold.
```

Conclusion: the deferred chunk enrichment path is useful as an experimental
toggle because it clearly improves locality signals, but it is **not yet a
strong enough end-to-end win to become the default**. Keep it available for
further experimentation, not as the retained default path.

Another follow-up scheduling experiment was tried after that locality work:

- keep `dynamic-cost-banded`, but let it optionally sample up to a small fixed
  number of base records per chunk before building the execution order so the
  cost model can include:
  - sampled `$ATTRIBUTE_LIST` density
  - sampled non-resident `$ATTRIBUTE_LIST` count
  - sampled records that still need enrichment
  - sampled referenced extension-record count

This was implemented as an **explicit opt-in benchmark toggle** rather than a
default behavior:

```powershell
$env:USN_RAW_MFT_BENCH_COST_HINT_ATTR_SAMPLE='1'
```

Exact-match profile runs on the same `C:` workload shape (`11` workers,
`2048` chunk records, `256 KiB` main buffer, `16 KiB` attribute buffer,
`summary-light`, offset-sorted extension loads) showed:

| Mode | Elapsed |
| --- | ---: |
| Dynamic | ~2.00 s |
| Dynamic cost-banded (default heuristic) | ~2.10 s |
| Dynamic cost-banded + sampled attr-list hints | ~2.40 s |

The Criterion confirmation sweep on the attr-list-sampled variant also regressed:

| Mode | Criterion result |
| --- | --- |
| Dynamic | `time: [1.9831 s 2.0198 s 2.0598 s]` |
| Dynamic cost-banded + sampled attr-list hints | `time: [2.4376 s 2.4758 s 2.5144 s]` |

Criterion reported:

```text
change: [+20.224% +22.255% +24.174%] (p = 0.00 < 0.05)
Performance has regressed.
```

Conclusion: the extra prepass does improve the scheduler's visibility into
extension-heavy chunks, but **the sampling overhead is too expensive on this
workload to justify enabling it by default**. Keep the telemetry fields and the
opt-in toggle for future experiments; retain the cheaper default cost model for
normal `dynamic-cost-banded` runs.

A follow-up refinement was also tried: keep the deferred attr-list idea, but
flush the deferred work every smaller record window instead of waiting for the
entire chunk to finish. A quick exact-match sweep suggested `256` records was
the most promising of the tested windows (`256` beat both whole-chunk deferred
and a `512`-record window on single-run elapsed time), so that variant was then
benchmarked against the saved legacy baseline.

| Mode | Criterion result |
| --- | --- |
| Windowed deferred attr-list (`256` records) | `time: [1.3494 s 1.3523 s 1.3557 s]` |

Criterion again reported only a sub-threshold improvement:

```text
change: [−1.5855% −0.8101% −0.1503%] (p = 0.04 < 0.05)
Change within noise threshold.
```

So windowing reduces some of the whole-chunk deferred path's phase-separation
cost, but it still does **not** turn the locality win into a strong enough
end-to-end speedup to replace the legacy path by default.

One more scheduling experiment was then tried at a higher level: keep the
existing plain per-record attr-list path, but change only the dynamic chunk
execution order so workers pull from a chunk list that has been presorted by
each chunk's physical start offset in the `$MFT` extent map. Results are still
yielded in original logical chunk order; only execution order changes.

A quick exact-match profile comparison on the `500000`-record workload slice did
not show a clear win. The profile run with `dynamic-physical-order` reduced some
attr-list read-time counters relative to one noisy `dynamic` run, but its total
elapsed time was slightly worse (`~1.457 s` vs `~1.442 s`). That was not enough
to trust the signal, so a full Criterion comparison was run on the larger full
benchmark shape.

| Mode | Criterion result |
| --- | --- |
| Plain dynamic | `time: [2.0440 s 2.0490 s 2.0547 s]` |
| Dynamic physical-order | `time: [2.0440 s 2.0509 s 2.0584 s]` |

Criterion reported:

```text
change: [−0.3416% +0.0965% +0.5266%] (p = 0.68 > 0.05)
No change in performance detected.
```

Conclusion: simply reordering dynamic chunk claims by each chunk's physical
start offset is **not** enough to beat or replace the current plain `dynamic`
default on this workload. If future scheduling work continues, it likely needs
to account for more than just the chunk start offset (for example chunk cost,
extent transitions inside the chunk, or multi-chunk bands) rather than only
presorting the work list.

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

### Follow-up ETW comparison: `dynamic` vs `contiguous` (May 2026)

After the benchmark defaults were retuned to the current best-known shape
(`11` workers, `2048` chunk records, `256 KiB` main buffer, `16 KiB`
attribute buffer), two matching ETW traces were captured with the same exact
profile executable and only the scheduling mode changed:

```powershell
$env:USN_RAW_MFT_BENCH_DRIVE='C'
$env:USN_RAW_MFT_BENCH_WORKERS='11'
$env:USN_RAW_MFT_BENCH_CHUNK_RECORDS='2048'
$env:USN_RAW_MFT_BENCH_BUFFER_BYTES='262144'
$env:USN_RAW_MFT_BENCH_ATTR_BUFFER_BYTES='16384'

$env:USN_RAW_MFT_BENCH_SCHEDULING='dynamic'
wpr -start GeneralProfile -start FileIO -start DiskIO -filemode
target\release\examples\raw_mft_parallel_ingest_profile.exe
wpr -stop tmp\raw_mft_parallel_ingest_validation\etw\raw_mft_parallel_ingest_dynamic.etl

$env:USN_RAW_MFT_BENCH_SCHEDULING='contiguous'
wpr -start GeneralProfile -start FileIO -start DiskIO -filemode
target\release\examples\raw_mft_parallel_ingest_profile.exe
wpr -stop tmp\raw_mft_parallel_ingest_validation\etw\raw_mft_parallel_ingest_contiguous.etl
```

Profile elapsed times from the matched runs:

| Scheduling | Profile elapsed |
| --- | ---: |
| Dynamic | ~2.692 s |
| Contiguous | ~4.410 s |

Quick `xperf -a diskio -detail` summaries for the profile process showed:

| Scheduling | Read count | Read MiB | Avg read I/O us | Avg read service us | Avg read QD start |
| --- | ---: | ---: | ---: | ---: | ---: |
| Dynamic | 58,693 | ~3228.0 | ~186.7 | ~38.0 | ~5.08 |
| Contiguous | 58,546 | ~3205.6 | ~154.7 | ~56.8 | ~2.21 |

So both runs issue almost the same total amount of read traffic, but they do not
exercise the disk in the same way:

- **dynamic** keeps more outstanding reads in flight
- **contiguous** shows a noticeably lower queue depth
- **contiguous** also shows a higher average disk service time per read

The coarse `xperf -a diskio` interval view points in the same direction.

Dynamic compressed the heavy read burst into roughly the `2-4 s` window:

- `2-3 s`: `96.24%` Disk 0 usage
- `3-4 s`: `90.67%` Disk 0 usage

Contiguous stayed busy for longer instead of finishing sooner:

- `2-3 s`: `94.47%` Disk 0 usage
- `3-4 s`: `95.00%` Disk 0 usage
- `4-5 s`: `64.95%` Disk 0 usage
- `5-6 s`: `30.08%` Disk 0 usage

That already suggests the main benefit of `dynamic` is not “better single-stream
sequentiality”, but better overlap and less idle tail time.

To test that more directly, the dominant raw-volume read stream for the profile
process (`\Device\HarddiskVolume2`) was sorted by request start time and the
byte-offset jump between consecutive reads was summarized:

| Scheduling | Exact sequential % | Jump <= 1 MiB % | Median abs jump |
| --- | ---: | ---: | ---: |
| Dynamic | `0.09%` | `3.91%` | ~`88.6 MiB` |
| Contiguous | `0.92%` | `7.63%` | ~`885.9 MiB` |

These numbers are noisy because requests from multiple workers are interleaved on
one time axis, and they show a mixed picture rather than one single locality
story:

- `contiguous` shows **more exact-adjacent and small-hop reads** than plain
  `dynamic`
- but it also shows a much larger median absolute jump, which is consistent with
  the global timeline switching between workers that own far-apart contiguous
  bands
- and it is still much slower overall

So the current evidence favors this interpretation:

1. `contiguous` improves some worker-local / short-hop locality signals
2. but that locality win is smaller than the wall-clock loss from chunk-cost
   imbalance and tail effects
3. `dynamic` wins because it keeps more useful work in flight and finishes the
   long tail sooner, even if its read offsets are globally more interleaved

This is exactly why the earlier batched-dynamic scheduling experiment was worth
trying: it targeted the gap between locality and balance. But because that
experiment did not produce a stable end-to-end win, the retained default still
should be plain `dynamic`.

### Formal ETW comparison: `dynamic` vs `contiguous` vs `dynamic-physical-order` (May 2026)

After wiring the capture script to also emit `xperf` text summaries, a follow-up
matched ETW run was captured for all three scheduling modes with the current
best-known workload shape:

- `11` workers
- `2048` chunk records
- `256 KiB` main buffer
- `16 KiB` attribute buffer
- `summary-light`
- offset-sorted `$ATTRIBUTE_LIST` extension loads

Command used:

```powershell
.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic,contiguous,dynamic-physical-order `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile `
  -SummarizeWithXperf `
  -SkipBuild `
  -OutputRoot .\tmp\raw_mft_parallel_ingest_validation\etw_formal
```

Capture outputs live under `tmp\raw_mft_parallel_ingest_validation\etw_formal`.

Profile elapsed times from the matched runs:

| Scheduling | Profile elapsed |
| --- | ---: |
| Dynamic | `2.413 s` |
| Contiguous | `3.714 s` |
| Dynamic physical order | `2.580 s` |

Process-scoped `xperf -a diskio -detail` summaries for the exact-match profile
process showed:

| Scheduling | Read count | Read MiB | Write MiB | Avg read I/O us | Avg read service us | Avg read QD start |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Dynamic | `57,421` | `3207.4` | `21.0` | `200.0` | `35.7` | `5.19` |
| Contiguous | `57,270` | `3185.2` | `16.2` | `167.6` | `50.8` | `2.59` |
| Dynamic physical order | `57,420` | `3207.7` | `11.6` | `229.0` | `37.4` | `5.41` |

The dominant file activity for the profile process was also consistent across
all three runs:

- essentially all read traffic was the raw-volume stream (`\Device\HarddiskVolume2`)
- most process write traffic was metadata churn against `C:\$Mft`
- `dynamic-physical-order` reduced process-side metadata write volume the most,
  but that did **not** translate into the best wall-clock result

The main interpretation from this stricter three-way ETW comparison is:

1. all three modes issue almost the same amount of read traffic, so this is not
   a “read fewer bytes” story
2. `contiguous` still loses badly on elapsed time even though its average per-read
   I/O completion time is lower, because it keeps much less work in flight
   (about half the average queue depth of `dynamic`)
3. `dynamic-physical-order` preserves the high queue depth of `dynamic`, but it
   still loses on elapsed time and shows the highest average end-to-end read I/O
   time of the three modes
4. the best current explanation is still that **plain dynamic scheduling wins by
   balancing chunk cost well enough to keep useful overlap high without adding
   enough locality damage to outweigh that benefit**

So the current retained default should remain:

- `dynamic` scheduling
- not `contiguous`
- and not the current `dynamic-physical-order` variant

### Repeated validation and first `dynamic-cost-banded` prototype (May 2026)

One more best-effort repeat was then run with the same tuned workload shape to
check whether the earlier `dynamic > contiguous` result collapses under a fresh
capture. The machine still was **not** fully quiet (the ETW process and file
summaries still showed background activity from `rustrover64.exe`, browsers,
`Doubao.exe`, `ArmouryCrate`, `docker.exe`, `java.exe`, `System` writes, and
cache/log churn), but the scheduling conclusion still held.

Repeated ETW pair:

| Scheduling | Profile elapsed |
| --- | ---: |
| Dynamic | `2.402 s` |
| Contiguous | `3.917 s` |

Repeated Criterion pair on the same workload shape:

| Scheduling | Result |
| --- | --- |
| Dynamic | `time: [2.0287 s 2.0641 s 2.1037 s]` |
| Contiguous | `time: [2.6833 s 2.8167 s 2.9640 s]` |

So even after another run with the same settings, `dynamic` still beats
`contiguous` by a wide enough margin that the result no longer looks like a
single-run accident.

That repeat also sharpened the current interpretation of the earlier ETW work:

1. `contiguous` can still lower some extension-read timing counters
2. but those lower extension-read counters still do **not** produce a lower wall-clock elapsed time
3. the dominant effect still looks like **load balance / overlap / long-tail control**, not simple local read ordering

With that in place, the next experiment was implemented as a first minimal
`dynamic-cost-banded` prototype rather than another pure locality reorder.

Current prototype shape:

- start from the physical-order chunk list
- split it into local bands of about four worker waves
- estimate chunk cost using static extent-map signals only (segment overlap,
  discontinuities, sparse holes, and physical span)
- front-load heavier chunks **inside each band**, but keep each concurrent claim
  wave physically ordered

This keeps the high-level `dynamic` execution model while testing whether a
cheap cost heuristic can reduce tail risk without falling all the way back to a
global physical reorder.

Initial validation for the prototype used the same workload shape as the current
best-known default (`11` workers, `2048` chunk records, `256 KiB` main buffer,
`16 KiB` attribute buffer, `summary-light`, offset-sorted extension loads).

Criterion summary table from the first run:

| Scheduling | Median elapsed |
| --- | ---: |
| Dynamic | `2.001 s` |
| Dynamic cost banded | `2.003 s` |
| Contiguous | `3.009 s` |

Criterion scheduling group results:

| Scheduling | Result |
| --- | --- |
| Dynamic | `time: [2.0124 s 2.0503 s 2.0896 s]` |
| Dynamic cost banded | `time: [2.0021 s 2.0331 s 2.0680 s]` |
| Contiguous | `time: [2.8068 s 2.8972 s 2.9867 s]` |

The first interpretation from Criterion is therefore:

- `dynamic-cost-banded` is **promising enough to keep as an experiment**
- but it is **not yet a statistically proven improvement** over plain `dynamic`
- it clearly avoids the large regression shape of `contiguous`

A matched ETW pair was then captured for `dynamic` and `dynamic-cost-banded`
with the same settings:

```powershell
.\scripts\capture-raw-mft-etw.ps1 `
  -Scheduling dynamic,dynamic-cost-banded `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -PrintAttrListProfile `
  -SummarizeWithXperf `
  -SkipBuild `
  -OutputRoot .\tmp\raw_mft_parallel_ingest_validation\etw_cost_banded
```

Profile elapsed times from that first matched pair were:

| Scheduling | Profile elapsed |
| --- | ---: |
| Dynamic | `2.464 s` |
| Dynamic cost banded | `2.362 s` |

The attr-list timing counters in the same pair also moved in the expected
direction for the prototype:

| Scheduling | `extension_load_ms` | `extension_record_read_ms` |
| --- | ---: | ---: |
| Dynamic | `10946.163` | `10708.573` |
| Dynamic cost banded | `9899.476` | `9656.243` |

But the jump/locality percentages themselves were essentially unchanged, so this
first prototype should be interpreted carefully:

1. it may already be moving some expensive chunk work earlier in a useful way
2. but the current static heuristic still has **too little evidence** behind it
   to replace plain `dynamic`
3. the next round should focus on better per-worker / per-chunk observability so
   future cost-model iterations can explain *why* a run helped or regressed

Current status after this first implementation wave:

- retain plain `dynamic` as the default
- keep `dynamic-cost-banded` as the active experiment branch for future tuning
- continue to treat pure physical-order locality work as lower priority than
  cost-aware tail reduction

To make that next tuning wave explainable, the exact-match profiling path now
also supports an opt-in scheduling profile (`USN_RAW_MFT_BENCH_PRINT_SCHEDULING_PROFILE=1`).
It prints:

- per-worker chunk counts / record counts / total chunk time
- the slowest completed chunks (tail candidates)
- the static cost estimate attached to each slow chunk
- for `dynamic-observed-adaptive`, each band's ordering source (`static` vs `observed`) and the sample count available before that band was prepared
- a simple predicted-vs-actual comparison signal, including each slow chunk's predicted rank and the overlap between the predicted-heaviest and actual-slowest top chunk lists
- recall-style mismatch summaries such as top-half hits, top-quarter hits, top-K misses, false positives, and the worst predicted rank among the actual tail chunks

The first run with that extra observability surfaced one more important finding:

- the current static cost heuristic now differentiates workers well enough to
  avoid being opaque in aggregate
- **but many of the slowest tail chunks on the measured `C:` volume still share
  the exact same estimated cost signature** (`2048` used records, no bitmap
  transitions, one contiguous extent segment)

That means the new observability is already useful, because it rules out one easy
explanation: the remaining tail is **not** coming from obviously sparse or
fragmented logical chunk bands. The next cost-model refinements likely need to
incorporate richer signals than extent-map geometry and coarse bitmap density
alone (for example sampled attr-list density, recent measured chunk cost, or
other metadata-derived predictors).

After adding bitmap-density signals and the new scheduling profile, the
prototype was benchmarked again with the same Criterion harness shape. The
result stayed in the same overall bucket:

| Scheduling | Result |
| --- | --- |
| Dynamic | `time: [1.9875 s 2.0243 s 2.0655 s]` |
| Dynamic cost banded | `time: [1.9912 s 2.0252 s 2.0636 s]` |

Interpretation: the observability work was worth keeping because it made the
tail behavior explainable, but the current heuristic revision still does **not**
produce a statistically proven win over plain `dynamic`. It remains an active
experiment branch, not a new default.

The next experiment branch after that result was **runtime feedback instead of
more prepass sampling**:

- keep the cheap chunk metadata already available from the extent map / bitmap
- start with the same local physical-band execution shape
- record real elapsed time for completed chunks at runtime
- reorder later bands from that observed cost signal instead of paying another
  prepass up front

That implementation now exists as a separate experimental scheduling mode:

```powershell
$env:USN_RAW_MFT_BENCH_SCHEDULING='dynamic-observed-adaptive'
```

A first full-workload evaluation was then run on the same tuned benchmark shape
(`11` workers, `2048` chunk records, `256 KiB` main buffer, `16 KiB` attribute
buffer, `summary-light`, offset-sorted extension loads).

The expanded one-shot summary table now prints the adaptive mismatch fields next
to elapsed time so quick local sweeps can capture both wall time and model
quality in one place:

| Workers | Scheduling | Median elapsed | Top-hit | Top-half | Top-quarter | Missed actual | Worst predicted rank |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 11 | Dynamic | `~2.216 s` | - | - | - | - | - |
| 11 | Dynamic observed-adaptive | `~2.266 s` | `37.50%` | `1/8` | `1/8` | `5` | `22` |

The matching Criterion scheduling sweep on the full workload produced:

| Scheduling | Criterion result |
| --- | --- |
| Dynamic | `time: [2.2414 s 2.2815 s 2.3235 s]` |
| Dynamic observed-adaptive | `time: [2.2280 s 2.2647 s 2.3080 s]` |

Interpretation of that pair:

- the observed-adaptive median was only about `0.7%` faster than plain
  `dynamic`
- the confidence intervals overlap heavily, so this is **not yet evidence of a
  stable win**
- it remains an experiment branch, not a new default

A matched exact-match profile pair on the same full workload shape showed almost
the same wall time, but the new mismatch telemetry still looked weak for the
current model:

| Scheduling | Profile elapsed | Top-hit | Top-half | Top-quarter | Missed actual | Worst predicted rank |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Dynamic | `~2.963 s` | - | - | - | - | - |
| Dynamic observed-adaptive | `~2.979 s` | `0/8` | `0/8` | `0/8` | `8` | `29` |

The adaptive run also showed `31` band decisions (`1` static, `30` observed),
so the runtime-feedback path really was engaged. But the slowest-chunk recall on
that matched run was still poor enough to matter:

- none of the actual slowest `8` chunks landed inside the predicted slowest `8`
- the worst actual tail chunk was only predicted at rank `29`
- one band produced an obvious outlier prediction spike (`~2384 ms` for chunk
  `245`) without turning that chunk into the dominant real tail event

So the current state of the runtime-feedback branch is:

- **promising enough to keep measuring** because it does not show the large
  regressions seen in `contiguous` or attr-list sampling
- **not yet explainable enough to trust as a replacement for `dynamic`**
  because the measured tail-recall metrics are still unstable and often weak

To reduce the worst instability in that branch, the next revision tightened the
runtime-feedback model itself before any new broad benchmark claims were made:

- require about **one full band** of completed samples before switching from the
  static heuristic to the observed model
- fit the observed model in **log elapsed-time space** instead of raw
  milliseconds so single slow samples do not explode later predictions
- clamp predicted milliseconds back into a range derived from the already
  observed sample distribution

Preliminary exact-match smoke runs on the same machine showed the intended
stability effect:

- the earlier multi-second prediction spike inside one adaptive band disappeared
  (for example the previously reported `~2384 ms` outlier prediction collapsed
  to a much smaller value in the same general region)
- `4`-worker / `40000`-record slice runs stayed entirely on static ordering
  after band `0` because only `13` completions were available before band `1`
  was prepared, which is below the new full-band sample threshold
- on the full `11`-worker workload, adaptive ordering now began at band `2`
  instead of band `1`, because band `1` still had only `34` completed samples
  when it needed to be prepared

These are **stability-oriented fixes**, not yet a new performance conclusion.
They make the observed-adaptive branch easier to reason about, but the branch
still needs another matched Criterion + profile pass before any stronger claim
about wall-clock improvement should be made.

One fresh post-stabilization rerun was then captured on the same full workload
shape (`11` workers, `2048` chunk records, `256 KiB` main buffer, `16 KiB`
attribute buffer, `summary-light`, offset-sorted extension loads).

The one-shot summary table printed:

| Workers | Scheduling | Median elapsed | Top-hit | Top-half | Top-quarter | Missed actual | Worst predicted rank |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 11 | Dynamic | `~2.214 s` | - | - | - | - | - |
| 11 | Dynamic cost-banded | `~2.644 s` | - | - | - | - | - |
| 11 | Dynamic observed-adaptive | `~2.214 s` | `62.50%` | `2/8` | `2/8` | `3` | `11` |

The matching Criterion scheduling sweep produced:

| Scheduling | Criterion result |
| --- | --- |
| Dynamic | `time: [2.1890 s 2.1938 s 2.1983 s]` |
| Dynamic cost-banded | `time: [2.6287 s 2.6332 s 2.6375 s]` |
| Dynamic observed-adaptive | `time: [2.1996 s 2.2060 s 2.2127 s]` |

Interpretation of that rerun:

- plain `dynamic` is still the fastest and most reliable mode in the measured
  data
- `dynamic-cost-banded` remains a clear regression on this workload (about
  `+20%` relative to plain `dynamic` in this rerun)
- the stabilized `dynamic-observed-adaptive` branch is now much closer to
  `dynamic`, but still **not a clear win**
- the same-run timing intervals show `dynamic` still edging out
  `dynamic-observed-adaptive` by a small margin; the branch is effectively in
  the "near-tie, no proven win" bucket right now

The matched exact-match profile trio on the same workload shape showed nearly
the same wall-clock ordering:

| Scheduling | Profile elapsed |
| --- | ---: |
| Dynamic | `~2.952 s` |
| Dynamic cost-banded | `~3.496 s` |
| Dynamic observed-adaptive | `~2.962 s` |

And the stabilized adaptive profile telemetry looked healthier than the earlier
unstable runs, but still not strong enough to justify promotion:

- adaptive ordering stayed static for bands `0` and `1`, then switched to
  observed at band `2`
- the earlier extreme prediction spikes were gone; observed-band predictions now
  stayed in a much narrower and more plausible range (roughly mid-single-digit
  to mid-teen milliseconds, with one larger but still bounded outlier around
  `16-18 ms` in most bands and about `75 ms` in one band)
- top-tail recall improved relative to the worst earlier run, but still only
  landed at `3/8` overlap in the matched exact profile and `5/8` misses

So the current state after the stabilization rerun is:

1. `dynamic` remains the retained default
2. `dynamic-cost-banded` remains a measured regression on this workload
3. `dynamic-observed-adaptive` is now a **stable experiment branch**, but still
   does **not** beat plain `dynamic` convincingly enough to graduate

Until stronger evidence appears, the retained conclusions do **not** change:

- plain `dynamic` remains the default
- `dynamic-cost-banded` remains experimental
- `dynamic-observed-adaptive` remains experimental
- attr-list sampling remains an explicit opt-in experiment only, because the
  measured result was a clear regression and should not be enabled by default

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

### Why performance improves sharply up to about 10-11 workers

The code gives a straightforward reason for the initial speedup:

- chunk execution is embarrassingly parallel at the API level
- each chunk is processed independently on a worker thread
- the main thread only drains results in order afterward

On the current `C:` workload there are about `1,329` planned chunks. That is
still plenty of parallel work for a moderate worker count, so scaling from
`1 -> 2 -> 4 -> 6 -> 8 -> 10 -> 11` reduces how much chunk work each worker
owns.

At `10-11` workers, the average chunk budget is still roughly `120-133` chunks
per worker, so there is enough work to keep workers busy while chunk overhead
stays amortized. The plateau is caused by the trade-off between useful
parallelism and the overhead/contention created by more workers.

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

- by roughly `10..=11` workers, the code has extracted most of the useful scan
  parallelism available from this workload
- beyond that point, extra workers mainly add contention on the same volume and
  more competing read/seek streams, so wall-clock time stops improving and then regresses

This matches the current measured curve too: improvement is strong through the
low worker counts, then the curve flattens around `10-11`, and `11` comes out
as the best measured point so far.

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

With about `1,329` planned chunks, contiguous scheduling is no longer harmed by
an explosion of tiny tasks, but `dynamic` still wins because the executor sees a
stream of uneven chunk costs. Dynamic workers keep consuming the next remaining
chunk, while contiguous workers stay stuck inside one preassigned band
regardless of how expensive that band becomes.

That matches the current result at `11` workers very well:

- `dynamic`: about `2.49 s`
- `contiguous`: about `3.67 s`

So the corrected benchmark did not invalidate the scheduling conclusion.
It changed the workload shape enough to move the worker optimum and then the
buffer sweeps shaved off another small but repeatable improvement.

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

1. Benchmark **`dynamic-observed-adaptive`** against `dynamic` and `dynamic-cost-banded` on the same Criterion harness shape, with scheduling profile output enabled for the exact-match runs.
2. Re-run the most promising comparison in a **quieter disk environment** so Criterion and ETW are measuring the ingest path instead of unrelated background writes.
3. Use **ETW/WPA** after Criterion finds a promising candidate to confirm whether a change reduced tail time, improved queue depth / overlap, or only rearranged reads.
4. If runtime feedback helps, iterate on band size / minimum-sample threshold before revisiting any heavier static prepass ideas.
5. Treat further string/link micro-optimizations and pure physical-order scheduling work as lower priority until the adaptive scheduling question is answered.

## Deferred-window follow-up after the fast-path cleanup

After the later base-record fast-path work (DOS-name materialization prefilter,
link-buffer reuse, visible-link filtering, disabled-profile timing cleanup, and
the legacy non-deferred batch fast path), the deferred chunk attr-list branch was
revisited with a narrower benchmark-active change: **do not buffer ready batch
entries in deferred mode until the current window has actually encountered a
deferred attr-list task**.

Before that change, the deferred path still pushed every ready entry into the
window's `entry_slots` buffer, even when the chunk had not yet seen any
attr-list work. On this workload, most records in a chunk are still plain ready
entries, so that meant the deferred branch was paying avoidable vector push /
drain overhead on the common fast path.

The implementation in `src\raw_mft\parallel\chunks.rs` now:

- hoists the current attr-list mode / flags into local variables once per chunk
- streams `ParsedBatchRecord::Ready` directly to the visitor while the current
  deferred window has no pending deferred tasks
- starts buffering only from the first deferred task onward, so ordered replay
  still works for the suffix that depends on deferred enrichment
- counts the deferred-window record budget only while there is pending deferred
  work to flush

Exact-match release-profile runs on the full-volume summary-light workload with
offset-sorted extension loads and a `256`-record deferred window gave:

| Mode | Elapsed |
| --- | --- |
| Deferred-window before change | `2.740 s` |
| Legacy per-record after change | `2.694 s` |
| Deferred-window after change | `2.671 s` |
| Deferred-window after change (repeat) | `2.700 s` |

Interpretation:

- the deferred branch clearly improved relative to its own immediate pre-change
  state on this machine / workload shape
- the same-state post-change comparison against legacy was much closer than the
  older deferred runs, and one matched run slightly favored deferred
- the repeat still landed in the same narrow band, so this now looks like a
  **promising near-tie / slight-win experiment branch**, not the clear regression
  it looked like before the streaming fix

The follow-up Criterion confirmation on the same harness shape used the current
legacy path as a saved baseline and then compared the tuned deferred-window path
against it:

| Mode | Criterion result |
| --- | --- |
| Legacy per-record baseline | `time: [2.6810 s 2.6914 s 2.7034 s]` |
| Deferred-window (`256`) vs legacy baseline | `time: [2.6637 s 2.6809 s 2.6983 s]` |

Criterion reported:

```text
change: [−1.1658% −0.3901% +0.3178%] (p = 0.36 > 0.05)
No change in performance detected.
```

That is still not enough evidence to replace the legacy path by default without
a retained default switch, but it does justify keeping the deferred window path
alive as a neutral experiment branch while the main optimization effort
continues to focus on the per-record fast path.

