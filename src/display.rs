//! Shared formatting helpers for the crate's bitflag newtypes.
//!
//! The USN reason, file-attribute, and source-info bitmasks all render the
//! same way: a list of set flag names joined by a separator, falling back to
//! `NONE` when no bits are set and to a hexadecimal value when only unknown
//! bits remain. Centralizing the logic here keeps the three `Display`
//! implementations consistent.

use std::fmt;

/// Write the names of the set flags in `bits`, or a `NONE`/hex fallback.
///
/// For every `(mask, name)` pair whose bits are fully present in `bits`, the
/// name is written, separated by `separator`. If no known flag matches, the
/// output is `NONE` when `bits == 0` and `0x{bits:x}` otherwise (so unknown
/// bits are still visible).
pub(crate) fn write_flag_names(
    f: &mut fmt::Formatter<'_>,
    bits: u32,
    names: &[(u32, &str)],
    separator: &str,
) -> fmt::Result {
    let mut wrote = false;
    for (mask, name) in names {
        if *mask != 0 && bits & *mask == *mask {
            if wrote {
                f.write_str(separator)?;
            }
            f.write_str(name)?;
            wrote = true;
        }
    }

    if wrote {
        Ok(())
    } else if bits == 0 {
        f.write_str("NONE")
    } else {
        write!(f, "0x{bits:x}")
    }
}
