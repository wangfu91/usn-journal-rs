# Raw MFT Serial Read Optimization Findings

## Summary

Optimized the serial `RawMft` read path (single-threaded, sequential record enumeration) through profiling and targeted improvements. **~10% throughput improvement** (888ms → 797ms median for 300k warm records on drive C).

### Linux follow-up (July 2026)

Criterion and `perf` were run against a read-only mounted Windows NTFS volume:

- 3,340,288 addressable records; 2,115,174 allocated records parsed
- baseline median: 1.090 s
- optimized median: 1.023 s
- change: **6.18% faster**, with identical counts and zero parse errors

The Linux profile placed `osstring_from_utf16le` at about 15% of sampled
cycles. The Linux-only path now constructs ASCII `OsString` bytes directly and
streams non-ASCII UTF-16 decoding without an intermediate `Vec<u16>`. Windows
retains `OsString::from_wide` so lone-surrogate/WTF-16 behavior is unchanged.

Use `USN_TEST_VOLUME=<mount-or-device>` for Linux Criterion runs.

## Windows baseline profiling

Established Criterion baseline using a 300k-record warm read on drive C (NTFS with typical WinSxS hard-linked files):
- **Median: 888ms** (~2.96µs per record)
- **Full-volume serial read: 8.9-9.2s warm** (~4.3µs/record on 2.06M records)

Flamegraph (cargo flamegraph on example target) revealed two dominant performance buckets:

### Bucket 1: File-Name Materialization (~30% of CPU time)
Key hotspots:
- `apply_file_name` → `osstring_from_utf16le`: ~15%
- UTF-16 → String conversion (Wtf8Buf transcode): ~8%
- `FileNameSelector` namespace/link processing: ~7%

**Root cause:** Each `$FILE_NAME` attribute was decoded from UTF-16LE to `OsString` via a wasteful two-step process:
1. Allocate `Vec<u16>` to hold UTF-16 code units
2. Pass to `OsString::from_wide`, which allocates again for the final string

For hard-linked files, this happened multiple times per record.

### Bucket 2: `$ATTRIBUTE_LIST` Extension Enrichment (~37% of CPU time)
Key hotspots:
- `enrich_from_attr_list`: 28%
- `with_loaded_extension_records`: 28%
- `read_record_raw` (building full extension entry): 24%
- Windows `ReadFile` API (random I/O): 27%

**Root cause:** NTFS spills attributes into extension records when FILE records overflow (common with heavily hard-linked files). The enrichment path:
1. Reads the `$ATTRIBUTE_LIST` (usually resident in base record)
2. For each unique extension record referenced, calls `read_record_raw` to build a complete `RawMftEntry`
3. Merges only specific fields (namespace, parent, file-name) back into the base entry

This is costly because:
- Extension records are scattered; each read may be a separate I/O
- `read_record_raw` parses the entire extension record and materializes all attributes
- Only a few fields are actually used from each extension

## Optimization #1: Eliminate Intermediate Vec in Name Decoding

**Result: ~10% improvement (888ms → 797ms)**

### Changes
- **Created `osstring_from_utf16le` helper** in `src/raw_mft/layout/attribute/view.rs`:
  - ASCII fast path: pure-ASCII names (valid UTF-8 byte-for-byte) build a `String` directly, then convert to `OsString::from(String)` (zero-copy on Windows)
  - Non-ASCII fallback: decode via `OsString::from_wide` to preserve lone surrogates
  - Returns `None` only for odd-length byte buffers
  
- **Changed `as_file_name()` signature**:
  - Old: returned `(NtfsFileNameHeader, Vec<u16>)`
  - New: returns `(NtfsFileNameHeader, &[u8])` (borrowed bytes from record buffer)
  
- **Updated name accessors**:
  - `name_units()` → `name_bytes()` (returns `&[u8]` instead of owned `Vec<u16>`)
  - ADS name accessors similarly return borrowed byte slices
  
- **Updated consumers** (`entry_build/entry.rs`, `entry_build/batch.rs`):
  - Call `osstring_from_utf16le(name_bytes)` instead of `OsString::from_wide(&name_units)`

### Benefits
- Eliminates one allocation per `$FILE_NAME` attribute (the intermediate `Vec<u16>`)
- ASCII fast path skips the surrogate-aware UTF-16 transcode loop
- Maintains correct semantics (preserves lone surrogates in non-ASCII names)
- Name decoding time reduced by ~15-20% of baseline

### Tradeoff
- Breaking change: `name_units()` signature changed (removed from public API per module design)
- Minor code complexity in fast path, but well-documented

## Optimization #2: Attempted FileNameSelector Allocation Reduction

**Result: No measurable improvement; reverted**

Explored reducing shrink-to-fit overhead and trying `Box<OsStr>` to reduce link storage. Neither showed improvement, likely because:
- Most records have 0-1 retained hard-link candidates, so allocation overhead is small
- The selector logic itself is a minor part of the total hotspot
- Reverted changes to maintain code clarity

## Optimization #3: `$ATTRIBUTE_LIST` Enrichment (Deferred)

The extension enrichment bucket (28% of CPU, 27% random I/O) is a larger architectural challenge:
- Optimizations would require:
  - Lean extension-record parser (only extract file-names, skip other attributes)
  - Per-extension record caching (avoid re-parsing the same extension multiple times)
  - Or making enrichment opt-in (changes API contract)
  
- Trade-offs:
  - Significant refactoring for modest gains (hard-linked files are a minority on most volumes)
  - I/O-bound in most cases (improving parsing won't help much)
  - Risk of regressing other workloads

**Recommendation:** Defer major refactor to a future optimization pass. Focus next on:
1. Batch MFT parsing (currently uses same enrichment path as serial)
2. Parallel ingest already has heavy optimizations in place
3. Monitor user workloads to assess whether hard-link enrichment cost justifies refactoring

## Profiling Methodology

- **Benchmark:** Criterion (0.8.2) with sample_size=20, warm_up=3s, measurement=30s
- **Profiling target:** `examples/raw_mft_serial_read_profile.rs` (exact-match workload)
- **Flamegraph:** cargo flamegraph (blondie/ETW backend, admin)
- **Environment:** Windows, admin privileges, drive C (NTFS with typical file distribution)

## Lessons

1. **UTF-16 handling on Windows is non-trivial:**
   - Pure-ASCII is common; ASCII fast path justifies code complexity
   - Surrogates must be preserved for non-ASCII names (WTF-8)
   
2. **Borrowed lifetimes reduce allocation pressure:**
   - Returning `&[u8]` from attribute accessors (tied to record buffer lifetime) eliminates intermediate allocations
   
3. **Hard-linked file enrichment has high I/O cost:**
   - WinSxS and other system directories create many hard links
   - Random reads for extension records saturate disk I/O bus
   - Consider per-workload tuning or making enrichment opt-in
   
4. **Profiling vs. benchmarking:**
   - Benchmarks show end-to-end throughput; flamegraphs show CPU distribution
   - I/O-bound sections (ReadFile 27%) don't improve much with CPU optimizations
   - Allocation-based optimizations are most effective for CPU time, not overall throughput

## Next steps

1. Re-profile after material changes to entry/link ownership; the remaining
   costs are distributed across required attribute walking and allocations.
2. Add rustdoc comments to key optimization points (osstring_from_utf16le, borrowed name accessors)
3. Consider full-volume re-benchmark to validate improvements at scale
4. Document attribute-list optimization opportunity for future maintainers
