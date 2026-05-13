# Raw MFT benchmark validation workflow

This note turns the current raw-`$MFT` ingest validation practice into a repeatable Windows workflow for **Criterion + repeated exact-match runs**.

## What this is for

Use this workflow when:

- you want confidence that a recent parser or scheduling micro-pass is a real retained win
- one-shot profile timings look promising, but you want a saved Criterion baseline too
- background disk activity has been noisy enough that you want a consistent pre-run “quiet disk” gate
- you want the same workload knobs applied to both:
  - `benches\raw_mft_ingest.rs`
  - `examples\raw_mft_parallel_ingest_profile.rs`

The helper script is:

- `scripts\measure-raw-mft-ingest.ps1`

## What the script does

For one chosen workload shape, the script:

1. creates a detached baseline worktree at `-BaselineRef`
2. waits for the disk to stay below a configurable throughput threshold before each measured step
3. runs a saved Criterion baseline in that baseline worktree
4. runs the current worktree against the same saved Criterion baseline
5. builds and runs the exact-match profile executable repeatedly for both baseline and current code
6. writes:
   - Criterion logs
   - per-run exact-match stdout / stderr
   - `summary.json`
   - a human-readable `README.md`

Outputs go under:

- `tmp\raw_mft_parallel_ingest_validation\measurement\...`

## Prerequisites

- Elevated PowerShell session
- `cargo` and `git` on `PATH`
- enough free time for the selected Criterion measurement window and exact-match repeat count

## Recommended starting shape

Unless you are testing a different hypothesis, start with the currently retained full-workload shape:

- `Scheduling=dynamic`
- `Workers=11`
- `ChunkRecords=2048`
- `MainBufferBytes=262144`
- `AttrBufferBytes=16384`
- `SummaryLight`
- `SortAttrListByOffset`

## Formal current-worktree vs `HEAD` comparison

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\measure-raw-mft-ingest.ps1 `
  -BaselineRef HEAD `
  -Scheduling dynamic `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -SampleSize 10 `
  -WarmUpSeconds 5 `
  -MeasurementSeconds 30 `
  -ExactRuns 5
```

This is the default “is the current uncommitted micro-pass real?” workflow.

## Compare against an older commit

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\measure-raw-mft-ingest.ps1 `
  -BaselineRef 1a7f3d8 `
  -Scheduling dynamic `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -SampleSize 12 `
  -WarmUpSeconds 5 `
  -MeasurementSeconds 45 `
  -ExactRuns 7
```

## Tighten the quiet-disk gate

If the machine is still noisy, require more consecutive quiet samples or a lower throughput threshold:

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\measure-raw-mft-ingest.ps1 `
  -BaselineRef HEAD `
  -Scheduling dynamic `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -QuietDiskThresholdMiBPerSec 16 `
  -QuietDiskConsecutiveSamples 4 `
  -QuietDiskSampleSeconds 2 `
  -QuietDiskTimeoutSeconds 180
```

The quiet-disk gate is best-effort. It helps avoid starting a run during obvious background bursts, but it does **not** guarantee a perfectly idle machine.

## Exact-match only smoke comparison

If you want the faster repeated profile check first and do not need Criterion yet:

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\measure-raw-mft-ingest.ps1 `
  -BaselineRef HEAD `
  -Scheduling dynamic `
  -Workers 11 `
  -ChunkRecords 2048 `
  -MainBufferBytes 262144 `
  -AttrBufferBytes 16384 `
  -SummaryLight `
  -SortAttrListByOffset `
  -ExactRuns 7 `
  -SkipCriterion
```

## Dry-run the plan

```powershell
Set-Location "D:\wangfu91\usn-journal-rs"

.\scripts\measure-raw-mft-ingest.ps1 `
  -BaselineRef HEAD `
  -DryRun
```

## Reading the outputs

Start with:

- `README.md` in the output folder for a quick summary
- `summary.json` if you want machine-readable results
- `criterion\baseline.log.txt`
- `criterion\current.log.txt`
- `exact-match\baseline-runs\run-XX\stdout.txt`
- `exact-match\current-runs\run-XX\stdout.txt`

Questions to ask:

1. Did Criterion report improvement, regression, noise-threshold, or no change?
2. Does the repeated exact-match median move in the same direction?
3. Is the median delta large enough to matter compared with the min/max spread?
4. If Criterion and repeated exact-match disagree, was the machine still noisy enough that ETW should be the next check?

## Practical guidance

- Keep the workload shape fixed while deciding whether a micro-pass is real.
- Prefer comparing one scheduling mode at a time.
- Use repeated exact-match runs as a fast confidence check, then keep Criterion as the statistical decision-maker.
- If a result is still marginal after this workflow, treat it as “not yet a retained win.”
- Use the ETW workflow in `docs\raw_mft_etw_workflow.md` after Criterion finds a promising candidate and you need to understand *why* it moved.

