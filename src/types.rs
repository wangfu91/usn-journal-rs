//! Strong newtype wrappers around the integer identifiers used by the
//! USN journal and the NTFS Master File Table.
//!
//! These types are `#[repr(transparent)]` over their underlying integer
//! representation, so wrapping or unwrapping is zero-cost.

use std::fmt;

/// Update Sequence Number — a monotonically increasing 64-bit signed
/// cursor into the USN change journal.
#[must_use]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Usn(pub i64);

impl Usn {
    /// Construct a `Usn` from its raw signed integer representation.
    #[inline]
    pub const fn new(v: i64) -> Self {
        Self(v)
    }

    /// Return the raw signed integer representation.
    #[inline]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl From<i64> for Usn {
    #[inline]
    fn from(v: i64) -> Self {
        Self(v)
    }
}

impl From<Usn> for i64 {
    #[inline]
    fn from(v: Usn) -> Self {
        v.0
    }
}

impl fmt::Display for Usn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// NTFS file reference number — a 64-bit value where the lower 48 bits
/// are the record number into the `$MFT` and the upper 16 bits are a
/// sequence number used for collision detection.
#[must_use]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Fid(pub u64);

impl Fid {
    /// Construct a `Fid` from its raw 64-bit representation.
    #[inline]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Return the raw 64-bit representation.
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Lower 48 bits — record number into the `$MFT`.
    #[inline]
    pub const fn record_number(self) -> u64 {
        self.0 & 0x0000_FFFF_FFFF_FFFF
    }

    /// Upper 16 bits — sequence number for collision detection.
    #[inline]
    pub const fn sequence(self) -> u16 {
        ((self.0 >> 48) & 0xFFFF) as u16
    }
}

impl From<u64> for Fid {
    #[inline]
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<Fid> for u64 {
    #[inline]
    fn from(v: Fid) -> Self {
        v.0
    }
}

impl fmt::Display for Fid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

bitflags::bitflags! {
    /// Strongly-typed view over an NTFS file-attribute bitmask
    /// (the value stored in `USN_RECORD_V2::FileAttributes`,
    /// `MftEntry::file_attributes`, and `RawMftEntry::si_file_attributes`).
    ///
    /// Mirrors the Win32 `FILE_ATTRIBUTE_*` constants. Unknown bits are
    /// preserved on round-trip via [`bitflags`]'s `from_bits_retain`.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FileAttributes: u32 {
        const READ_ONLY            = 0x0000_0001;
        const HIDDEN               = 0x0000_0002;
        const SYSTEM               = 0x0000_0004;
        const DIRECTORY            = 0x0000_0010;
        const ARCHIVE              = 0x0000_0020;
        const DEVICE               = 0x0000_0040;
        const NORMAL               = 0x0000_0080;
        const TEMPORARY            = 0x0000_0100;
        const SPARSE_FILE          = 0x0000_0200;
        const REPARSE_POINT        = 0x0000_0400;
        const COMPRESSED           = 0x0000_0800;
        const OFFLINE              = 0x0000_1000;
        const NOT_CONTENT_INDEXED  = 0x0000_2000;
        const ENCRYPTED            = 0x0000_4000;
        const INTEGRITY_STREAM     = 0x0000_8000;
        const VIRTUAL              = 0x0001_0000;
        const NO_SCRUB_DATA        = 0x0002_0000;
        const RECALL_ON_OPEN       = 0x0004_0000;
        const RECALL_ON_DATA_ACCESS = 0x0040_0000;
    }
}

bitflags::bitflags! {
    /// Strongly-typed view over a USN reason bitmask (the value stored
    /// in `USN_RECORD_V2::Reason` and `UsnEntry::reason`).
    ///
    /// Mirrors the Win32 `USN_REASON_*` constants. Unknown bits are
    /// preserved on round-trip via [`bitflags`]'s `from_bits_retain`.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct UsnReason: u32 {
        const DATA_OVERWRITE          = 0x0000_0001;
        const DATA_EXTEND             = 0x0000_0002;
        const DATA_TRUNCATION         = 0x0000_0004;
        const NAMED_DATA_OVERWRITE    = 0x0000_0010;
        const NAMED_DATA_EXTEND       = 0x0000_0020;
        const NAMED_DATA_TRUNCATION   = 0x0000_0040;
        const FILE_CREATE             = 0x0000_0100;
        const FILE_DELETE             = 0x0000_0200;
        const EA_CHANGE               = 0x0000_0400;
        const SECURITY_CHANGE         = 0x0000_0800;
        const RENAME_OLD_NAME         = 0x0000_1000;
        const RENAME_NEW_NAME         = 0x0000_2000;
        const INDEXABLE_CHANGE        = 0x0000_4000;
        const BASIC_INFO_CHANGE       = 0x0000_8000;
        const HARD_LINK_CHANGE        = 0x0001_0000;
        const COMPRESSION_CHANGE      = 0x0002_0000;
        const ENCRYPTION_CHANGE       = 0x0004_0000;
        const OBJECT_ID_CHANGE        = 0x0008_0000;
        const REPARSE_POINT_CHANGE    = 0x0010_0000;
        const STREAM_CHANGE           = 0x0020_0000;
        const TRANSACTED_CHANGE       = 0x0040_0000;
        const INTEGRITY_CHANGE        = 0x0080_0000;
        const DESIRED_STORAGE_CLASS_CHANGE = 0x0100_0000;
        const CLOSE                   = 0x8000_0000;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usn_round_trip() {
        let v = Usn::new(0x1234_5678);
        assert_eq!(v.get(), 0x1234_5678);
        let i: i64 = v.into();
        assert_eq!(i, 0x1234_5678);
        assert_eq!(Usn::from(0x42i64).get(), 0x42);
    }

    #[test]
    fn usn_display_is_decimal() {
        assert_eq!(format!("{}", Usn::new(0x10)), "16");
    }

    #[test]
    fn fid_record_and_sequence() {
        let raw: u64 = (0xABCDu64 << 48) | 0x0000_0000_0000_002A;
        let fid = Fid::new(raw);
        assert_eq!(fid.record_number(), 0x2A);
        assert_eq!(fid.sequence(), 0xABCD);
        assert_eq!(fid.get(), raw);
    }

    #[test]
    fn fid_display_is_hex() {
        assert_eq!(format!("{}", Fid::new(0x10)), "0x10");
    }

    #[test]
    fn fid_round_trip() {
        let v = Fid::new(0xDEAD_BEEF);
        let u: u64 = v.into();
        assert_eq!(u, 0xDEAD_BEEF);
        assert_eq!(Fid::from(0x42u64).get(), 0x42);
    }
}
