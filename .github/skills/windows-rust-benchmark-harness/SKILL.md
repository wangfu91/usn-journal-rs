---
name: windows-rust-benchmark-harness
description: "Build trustworthy Rust benchmark harnesses on Windows. Use for Criterion benchmarks, converting noisy harnesses, exact-match profiling targets, baseline comparisons, benchmark-noise control, and validating whether a diff is a real performance win or regression."
---

# Windows Rust Benchmark Harness

Use this skill when you need a **repeatable Windows-native Rust benchmark workflow** instead of ad-hoc timing runs.

## When to Use

- Build or replace a benchmark harness for parser, ingest, indexing, or path-resolution workloads
- Convert a noisy harness to **Criterion**
- Create an exact-match profiling target that shares logic with the benchmark
- Validate whether an optimization is real before keeping it
- Compare current code against a saved baseline or against `HEAD` with the same harness

## Procedure

1. Benchmark the **real workload shape**, not a simplified proxy, unless you are intentionally isolating a micro-hotspot.
2. Prefer **Criterion** when milliseconds matter and you need confidence intervals, outlier reporting, and baseline comparisons.
3. Move the workload logic into a **shared support module** used by both:
   - the Criterion bench target
   - the profiling example or executable
4. Make the profiling target **behaviorally identical** to the benchmarked path so flamegraph and ETW results do not drift.
5. Use explicit benchmark controls:
   - `sample_size`
   - `warm_up_time`
   - `measurement_time`
   - `--save-baseline <name>`
   - `--baseline <name>`
6. Record the benchmark configuration in output:
   - worker count
   - chunk size
   - buffer sizes
   - dataset range
   - important environment variables
7. Validate a performance-sensitive diff in one of two ways:
   - compare against a saved Criterion baseline
   - or benchmark `HEAD` with the **same current harness** in a detached worktree
8. Keep only changes that are measurably faster on the real workload.

## Important Notices

- Prefer **Criterion** over Divan when you need statistically useful output for noisy Windows workloads.
- Do not let the benchmark and profile target diverge; shared workload code is the safest pattern.
- Keep benchmark-only changes separate from parser/runtime changes when possible so you can validate them independently.
- Windows background I/O can distort results; repeated measurements and baseline comparisons matter.

## References

- [Criterion workflow](./references/criterion-workflow.md)

