# Flamegraph Workflow

## Basic Command

```powershell
cargo flamegraph -o profile.svg --example <exact-match-profile-target>
```

Use a benchmark-matching example whenever possible.

## What Hotspots Usually Mean

### String / UTF conversion hot

Examples:

- `OsString::from_wide`
- `to_string_lossy`
- `Wtf8Buf`

Likely direction:

- reduce conversions
- delay materialization
- stop cloning names that are thrown away later

### Allocation / clone hot

Examples:

- `Vec::push`
- `RawVec::grow_one`
- `clone`
- `extend_from_slice`

Likely direction:

- reserve more accurately
- avoid building data you immediately fold away
- reduce repeated merge/copy work

### Parser / validation hot

Examples:

- record parse
- fixup
- attribute walking

Likely direction:

- tighten hot loops
- reuse buffers or cursors
- reduce repeated decoding

### Raw seek / read hot

Examples:

- `SetFilePointerEx`
- `ReadFile`
- custom reader refill / seek methods

Likely direction:

- improve I/O locality
- adjust scheduling or chunk order
- use ETW to confirm disk-bound behavior

## Decision Rule

- If the flamegraph is dominated by CPU and allocation frames, optimize code and data structures.
- If the flamegraph already shows lots of raw I/O calls, do not assume the next win is in CPU code; collect ETW.

