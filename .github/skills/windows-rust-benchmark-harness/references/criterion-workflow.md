# Criterion Workflow for Windows Rust Projects

## Recommended Pattern

1. Put the real workload in a shared Rust module under `benches/support/` or another internal helper location.
2. Make the Criterion bench a thin wrapper around that shared workload.
3. Make the profiling example call the **same shared workload**.

## Criterion Settings That Usually Help

Use settings like:

- `sample_size(10)` for expensive multi-second runs
- `warm_up_time(Duration::from_secs(3))`
- `measurement_time(Duration::from_secs(30))`
- `.configure_from_args()` so CLI flags still work

Tune them for the workload rather than copying blindly.

## Useful Commands

```powershell
cargo bench --bench <bench-name> -- --sample-size 10
cargo bench --bench <bench-name> -- --save-baseline before-change
cargo bench --bench <bench-name> -- --baseline before-change
```

If you need to compare parser/runtime code while keeping the benchmark harness fixed:

1. Create a detached worktree at `HEAD`
2. Copy the **current** benchmark harness files into that worktree
3. Run the same Criterion command in both worktrees

This isolates runtime changes from harness changes.

## What to Record

Always print or note:

- benchmark target name
- worker count / thread count
- chunk size / batch size
- buffer sizes
- input range or dataset scope
- relevant environment variables
- Criterion result interval

## Interpreting Criterion Output

- `time: [low mid high]` is the confidence interval summary
- `change: [...]` shows the measured delta against the chosen baseline
- `p = 0.00 < 0.05` means the change is statistically significant under Criterion's model

Practical guidance:

- If the interval meaningfully overlaps prior results and noise is high, do not claim a win yet
- If the workload is I/O heavy, even a significant result should be checked across multiple runs or a quieter system state

## Noise Control

- Run on the target OS and storage type used by production or user workflows
- Close obviously noisy applications when possible
- Avoid mixing compilation time with runtime measurement
- Prefer full-release or bench profiles
- If disk pressure dominates, move on to ETW instead of chasing CPU micro-optimizations

