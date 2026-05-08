---
name: windows-rust-flamegraph-profiling
description: "Profile Windows Rust code with cargo flamegraph. Use for exact-match profiling targets, hotspot hunting, allocation and clone analysis, parser-loop investigation, and deciding whether the next optimization should target CPU work, allocations, or data-structure churn."
---

# Windows Rust Flamegraph Profiling

Use this skill when the benchmark says there is a problem, but you still need to learn **where CPU time is going**.

## When to Use

- A Criterion benchmark is slow or regressed
- You need to find hot functions, clones, allocations, or parsing loops
- You want an exact-match profiling target for a benchmarked workload
- You need to validate whether a hotspot is inside the benchmarked code or just harness noise

## Procedure

1. Start from the **same workload logic** used by the benchmark.
2. Prefer profiling a dedicated example or executable over profiling the benchmark harness directly.
3. Run `cargo flamegraph` on that exact-match target.
4. Extract the hottest frames and group them into a few buckets:
   - string / path / UTF conversion
   - allocation / growth / clone
   - parser / decode / validation
   - synchronization / channel / scheduling
   - raw I/O calls
5. Map the top buckets back to source lines before changing code.
6. After a change, re-benchmark before trusting the flamegraph-driven hypothesis.

## Important Notices

- Flamegraphs show **CPU time**, not end-to-end storage contention. If raw I/O dominates, move to ETW.
- An optimization that removes a hot frame can still regress overall throughput if it adds more overhead elsewhere.
- Do not profile a workload that differs from the real benchmark path; that causes false conclusions.

## References

- [Flamegraph workflow](./references/flamegraph-workflow.md)

