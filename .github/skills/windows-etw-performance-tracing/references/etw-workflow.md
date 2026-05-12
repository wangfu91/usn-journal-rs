# ETW Workflow and Interpretation

## Symbol Setup

Set a symbol path before deeper stack analysis:

```powershell
$env:_NT_SYMBOL_PATH = 'srv*C:\symbols*https://msdl.microsoft.com/download/symbols'
```

## Recommended First Capture

```powershell
wpr -start GeneralProfile -start FileIO -start DiskIO -filemode
cargo run --release --example <exact-match-profile-target>
wpr -stop trace.etl
```

Use the workload-matching profile target, not a different example.

## Useful xperf Summaries

```powershell
xperf -i trace.etl -a diskio
xperf -i trace.etl -a cpudisk
xperf -i trace.etl -a process
xperf -i trace.etl -a filename
```

Then open the same trace in WPA for timeline analysis.

## What to Look For

### Disk nearly saturated during the hot window

If disk utilization sits around `90%+` during the workload burst:

- the benchmark is at least partly storage-bound
- parser micro-optimizations may have limited value
- chunk scheduling and read locality become more promising

### Heavy background `System` writes or indexer activity

If `System`, indexing, antivirus, or unrelated tools are moving lots of bytes:

- do not over-interpret a single benchmark run
- re-run in a quieter state
- treat small wins or regressions cautiously

### Mostly read-heavy workload, but poor service time

If reads dominate but service time is high:

- investigate seek patterns
- reduce fragmentation in work assignment
- keep worker ranges more contiguous

### Low disk pressure but high CPU cost

If disk utilization is modest and CPU work dominates:

- go back to flamegraph
- attack allocations, clones, parsing loops, or synchronization

## When to Use WPA

Open WPA when CLI summaries are not enough to answer:

- which process caused the busiest I/O interval
- whether queue depth spikes line up with throughput drops
- whether the hot window is dominated by your process or by background noise
- whether read locality is poor across worker threads

## Optional Advanced Trace

If you need call stacks for disk and CPU behavior, use a custom `xperf` stackwalk capture. Keep this as a second step after WPR unless you already know you need it.

