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

/// File identifier returned by the USN journal and MFT enumeration APIs.
///
/// NTFS uses a 64-bit file reference number, where the lower 48 bits are
/// the record number into the `$MFT` and the upper 16 bits are a sequence
/// number used for collision detection. ReFS uses 128-bit file IDs in
/// `USN_RECORD_V3`.
#[must_use]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Fid {
    /// Standard NTFS 64-bit file reference number.
    Standard(u64),
    /// ReFS / USN v3 128-bit file identifier.
    Extended(u128),
}

impl Default for Fid {
    #[inline]
    fn default() -> Self {
        Self::Standard(0)
    }
}

impl Fid {
    /// Mask for the lower 48 bits (record number) of a standard NTFS file reference.
    const RECORD_NUMBER_MASK: u64 = (1u64 << 48) - 1;
    /// Mask for the upper 16 bits (sequence number) of a standard NTFS file reference.
    const SEQUENCE_MASK: u64 = 0xFFFF;
    /// Bit offset of the sequence number within a standard NTFS file reference.
    const SEQUENCE_SHIFT: u32 = 48;

    /// Construct a standard 64-bit NTFS file reference number.
    #[inline]
    pub const fn new(v: u64) -> Self {
        Self::Standard(v)
    }

    /// Construct a 128-bit file identifier.
    #[inline]
    pub const fn from_u128(v: u128) -> Self {
        Self::Extended(v)
    }

    /// Construct a 128-bit file identifier from raw little-endian bytes.
    #[inline]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self::Extended(u128::from_le_bytes(bytes))
    }

    /// Returns `true` if this is a standard NTFS 64-bit file reference.
    #[inline]
    pub const fn is_standard(self) -> bool {
        matches!(self, Self::Standard(_))
    }

    /// Returns `true` if this is a 128-bit file identifier.
    #[inline]
    pub const fn is_extended(self) -> bool {
        matches!(self, Self::Extended(_))
    }

    /// Return the raw 64-bit representation when this is a standard NTFS ID.
    #[inline]
    pub const fn as_u64(self) -> Option<u64> {
        match self {
            Self::Standard(v) => Some(v),
            Self::Extended(_) => None,
        }
    }

    /// Return the raw 128-bit representation.
    ///
    /// Standard 64-bit IDs are zero-extended.
    #[inline]
    pub const fn as_u128(self) -> u128 {
        match self {
            Self::Standard(v) => v as u128,
            Self::Extended(v) => v,
        }
    }

    /// Return the raw little-endian bytes of this identifier.
    #[inline]
    pub const fn as_bytes(self) -> [u8; 16] {
        self.as_u128().to_le_bytes()
    }

    /// Lower 48 bits of a standard NTFS ID — the record number into `$MFT`.
    ///
    /// Returns `None` for 128-bit file IDs because the concept is NTFS-specific.
    #[inline]
    pub const fn record_number(self) -> Option<u64> {
        match self {
            Self::Standard(v) => Some(v & Self::RECORD_NUMBER_MASK),
            Self::Extended(_) => None,
        }
    }

    /// Upper 16 bits of a standard NTFS ID — sequence number for collision detection.
    ///
    /// Returns `None` for 128-bit file IDs because the concept is NTFS-specific.
    #[inline]
    pub const fn sequence(self) -> Option<u16> {
        match self {
            Self::Standard(v) => Some(((v >> Self::SEQUENCE_SHIFT) & Self::SEQUENCE_MASK) as u16),
            Self::Extended(_) => None,
        }
    }
}

impl From<u64> for Fid {
    #[inline]
    fn from(v: u64) -> Self {
        Self::Standard(v)
    }
}

impl From<u128> for Fid {
    #[inline]
    fn from(v: u128) -> Self {
        Self::Extended(v)
    }
}

impl TryFrom<Fid> for u64 {
    type Error = &'static str;

    #[inline]
    fn try_from(v: Fid) -> Result<Self, Self::Error> {
        v.as_u64()
            .ok_or("128-bit file identifiers cannot be represented as u64")
    }
}

impl fmt::Display for Fid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Standard(v) => write!(f, "0x{v:x}"),
            Self::Extended(v) => write!(f, "0x{v:x}"),
        }
    }
}

bitflags::bitflags! {
    /// Strongly-typed view over an NTFS file-attribute bitmask
    /// (the value stored in `USN_RECORD_V2::FileAttributes`,
    /// `USN_RECORD_V3::FileAttributes`,
    /// `MftEntry::file_attributes`, and `RawMftEntry::si_file_attributes`).
    ///
    /// Mirrors the Win32 `FILE_ATTRIBUTE_*` constants. Unknown bits are
    /// preserved on round-trip via [`bitflags`]'s `from_bits_retain`.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FileAttributes: u32 {
        /// Item is read-only.
        const READ_ONLY            = 0x0000_0001;
        /// Item is hidden from normal directory listings.
        const HIDDEN               = 0x0000_0002;
        /// Item is used by the operating system.
        const SYSTEM               = 0x0000_0004;
        /// Item is a directory.
        const DIRECTORY            = 0x0000_0010;
        /// Item should be archived.
        const ARCHIVE              = 0x0000_0020;
        /// Reserved device attribute.
        const DEVICE               = 0x0000_0040;
        /// Item has no other special attributes set.
        const NORMAL               = 0x0000_0080;
        /// Item should preferably be kept in temporary storage.
        const TEMPORARY            = 0x0000_0100;
        /// Item contains sparse data.
        const SPARSE_FILE          = 0x0000_0200;
        /// Item is represented by a reparse point.
        const REPARSE_POINT        = 0x0000_0400;
        /// Item is compressed on disk.
        const COMPRESSED           = 0x0000_0800;
        /// Item's data is not immediately available.
        const OFFLINE              = 0x0000_1000;
        /// Item should not be content indexed.
        const NOT_CONTENT_INDEXED  = 0x0000_2000;
        /// Item is encrypted on disk.
        const ENCRYPTED            = 0x0000_4000;
        /// Item uses integrity streams.
        const INTEGRITY_STREAM     = 0x0000_8000;
        /// Reserved virtual attribute.
        const VIRTUAL              = 0x0001_0000;
        /// Item is excluded from scrubber processing.
        const NO_SCRUB_DATA        = 0x0002_0000;
        /// Item is recalled when opened.
        const RECALL_ON_OPEN       = 0x0004_0000;
        /// Item is recalled on data access.
        const RECALL_ON_DATA_ACCESS = 0x0040_0000;
    }
}

bitflags::bitflags! {
    /// Strongly-typed view over a USN reason bitmask (the value stored
    /// in `USN_RECORD_V2::Reason`, `USN_RECORD_V3::Reason`, and `UsnEntry::reason`).
    ///
    /// Mirrors the Win32 `USN_REASON_*` constants. Unknown bits are
    /// preserved on round-trip via [`bitflags`]'s `from_bits_retain`.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct UsnReason: u32 {
        /// Unnamed stream data was overwritten.
        const DATA_OVERWRITE          = 0x0000_0001;
        /// Unnamed stream data was extended.
        const DATA_EXTEND             = 0x0000_0002;
        /// Unnamed stream data was truncated.
        const DATA_TRUNCATION         = 0x0000_0004;
        /// Named stream data was overwritten.
        const NAMED_DATA_OVERWRITE    = 0x0000_0010;
        /// Named stream data was extended.
        const NAMED_DATA_EXTEND       = 0x0000_0020;
        /// Named stream data was truncated.
        const NAMED_DATA_TRUNCATION   = 0x0000_0040;
        /// A file or directory was created.
        const FILE_CREATE             = 0x0000_0100;
        /// A file or directory was deleted.
        const FILE_DELETE             = 0x0000_0200;
        /// Extended attributes changed.
        const EA_CHANGE               = 0x0000_0400;
        /// Security descriptors changed.
        const SECURITY_CHANGE         = 0x0000_0800;
        /// An old name in a rename operation.
        const RENAME_OLD_NAME         = 0x0000_1000;
        /// A new name in a rename operation.
        const RENAME_NEW_NAME         = 0x0000_2000;
        /// Indexing-related metadata changed.
        const INDEXABLE_CHANGE        = 0x0000_4000;
        /// Basic file metadata changed.
        const BASIC_INFO_CHANGE       = 0x0000_8000;
        /// A hard-link set changed.
        const HARD_LINK_CHANGE        = 0x0001_0000;
        /// Compression state changed.
        const COMPRESSION_CHANGE      = 0x0002_0000;
        /// Encryption state changed.
        const ENCRYPTION_CHANGE       = 0x0004_0000;
        /// Object ID metadata changed.
        const OBJECT_ID_CHANGE        = 0x0008_0000;
        /// Reparse-point metadata changed.
        const REPARSE_POINT_CHANGE    = 0x0010_0000;
        /// Stream topology changed.
        const STREAM_CHANGE           = 0x0020_0000;
        /// A transacted operation changed the file.
        const TRANSACTED_CHANGE       = 0x0040_0000;
        /// Integrity metadata changed.
        const INTEGRITY_CHANGE        = 0x0080_0000;
        /// Desired storage class changed.
        const DESIRED_STORAGE_CLASS_CHANGE = 0x0100_0000;
        /// The handle that caused the change was closed.
        const CLOSE                   = 0x8000_0000;
    }
}

bitflags::bitflags! {
    /// Strongly-typed view over a USN source-info bitmask.
    ///
    /// Mirrors the Win32 `USN_SOURCE_*` constants. Unknown bits are preserved
    /// on round-trip via [`bitflags`]'s `from_bits_retain`.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct UsnSourceInfo: u32 {
        /// The change was caused by data-management software.
        const DATA_MANAGEMENT = 0x0000_0001;
        /// The change was caused by auxiliary data management.
        const AUXILIARY_DATA = 0x0000_0002;
        /// The change was caused by replication management.
        const REPLICATION_MANAGEMENT = 0x0000_0004;
        /// The change was caused by client-side replication management.
        const CLIENT_REPLICATION_MANAGEMENT = 0x0000_0008;
    }
}

/// Display names for known `USN_SOURCE_*` bits.
const SOURCE_INFO_NAMES: &[(UsnSourceInfo, &str)] = &[
    (UsnSourceInfo::DATA_MANAGEMENT, "DATA_MANAGEMENT"),
    (UsnSourceInfo::AUXILIARY_DATA, "AUXILIARY_DATA"),
    (
        UsnSourceInfo::REPLICATION_MANAGEMENT,
        "REPLICATION_MANAGEMENT",
    ),
    (
        UsnSourceInfo::CLIENT_REPLICATION_MANAGEMENT,
        "CLIENT_REPLICATION_MANAGEMENT",
    ),
];

impl fmt::Display for UsnSourceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote = false;
        for (flag, name) in SOURCE_INFO_NAMES {
            if self.contains(*flag) {
                if wrote {
                    f.write_str(" | ")?;
                }
                f.write_str(name)?;
                wrote = true;
            }
        }
        if wrote { Ok(()) } else { f.write_str("NONE") }
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
        assert_eq!(fid.record_number(), Some(0x2A));
        assert_eq!(fid.sequence(), Some(0xABCD));
        assert_eq!(fid.as_u64(), Some(raw));
    }

    #[test]
    fn fid_display_is_hex() {
        assert_eq!(format!("{}", Fid::new(0x10)), "0x10");
    }

    #[test]
    fn fid_round_trip() {
        let v = Fid::new(0xDEAD_BEEF);
        let u: u64 = v.try_into().expect("standard fid");
        assert_eq!(u, 0xDEAD_BEEF);
        assert_eq!(Fid::from(0x42u64).as_u64(), Some(0x42));
    }

    #[test]
    fn extended_fid_round_trip() {
        let fid = Fid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_11c6);
        assert!(fid.is_extended());
        assert_eq!(
            fid.as_bytes(),
            [
                0xc6, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ]
        );
        assert_eq!(format!("{fid}"), "0x11c6");
        assert_eq!(fid.record_number(), None);
        assert_eq!(fid.sequence(), None);
        assert!(u64::try_from(fid).is_err());
    }

    #[test]
    fn extended_fid_from_bytes_round_trip() {
        let raw = [
            1, 2, 3, 4, 5, 6, 7, 8, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x10, 0x20,
        ];
        let fid = Fid::from_bytes(raw);
        assert_eq!(fid.as_bytes(), raw);
    }
}
