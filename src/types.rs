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
    /// The zero USN — the conventional starting cursor for a full journal or
    /// MFT scan.
    pub const ZERO: Self = Self(0);

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

    /// Returns `true` if this is the zero USN (the conventional scan start).
    #[must_use]
    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Advance this USN by `delta`, saturating at the numeric bounds.
    ///
    /// USNs are byte offsets into the change journal, so advancing by a byte
    /// delta is occasionally useful when resuming a scan.
    #[inline]
    pub const fn saturating_add(self, delta: i64) -> Self {
        Self(self.0.saturating_add(delta))
    }

    /// Advance this USN by `delta`, returning `None` on overflow.
    #[inline]
    pub const fn checked_add(self, delta: i64) -> Option<Self> {
        match self.0.checked_add(delta) {
            Some(value) => Some(Self(value)),
            None => None,
        }
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

    /// Construct a standard NTFS file reference from its 48-bit record number
    /// and 16-bit sequence number — the inverse of [`Self::record_number`] and
    /// [`Self::sequence`].
    ///
    /// The record number is masked to its low 48 bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use usn_journal_rs::Fid;
    ///
    /// let fid = Fid::from_parts(0x2a, 0xabcd);
    /// assert_eq!(fid.record_number(), Some(0x2a));
    /// assert_eq!(fid.sequence(), Some(0xabcd));
    /// ```
    #[inline]
    pub const fn from_parts(record_number: u64, sequence: u16) -> Self {
        Self::Standard(
            ((sequence as u64) << Self::SEQUENCE_SHIFT) | (record_number & Self::RECORD_NUMBER_MASK),
        )
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

impl fmt::LowerHex for Fid {
    /// Bare lowercase hex of the underlying identifier. Honors the `#` flag for
    /// the `0x` prefix, so `format!("{:#x}", fid)` matches the `Display` form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Standard(v) => fmt::LowerHex::fmt(v, f),
            Self::Extended(v) => fmt::LowerHex::fmt(v, f),
        }
    }
}

impl fmt::UpperHex for Fid {
    /// Bare uppercase hex of the underlying identifier. Honors the `#` flag for
    /// the `0X` prefix.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Standard(v) => fmt::UpperHex::fmt(v, f),
            Self::Extended(v) => fmt::UpperHex::fmt(v, f),
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

impl FileAttributes {
    /// Returns `true` if the directory attribute is set.
    #[must_use]
    #[inline]
    pub fn is_directory(self) -> bool {
        self.contains(Self::DIRECTORY)
    }

    /// Returns `true` if the read-only attribute is set.
    #[must_use]
    #[inline]
    pub fn is_read_only(self) -> bool {
        self.contains(Self::READ_ONLY)
    }

    /// Returns `true` if the hidden attribute is set.
    #[must_use]
    #[inline]
    pub fn is_hidden(self) -> bool {
        self.contains(Self::HIDDEN)
    }

    /// Returns `true` if the system attribute is set.
    #[must_use]
    #[inline]
    pub fn is_system(self) -> bool {
        self.contains(Self::SYSTEM)
    }

    /// Returns `true` if the archive attribute is set.
    #[must_use]
    #[inline]
    pub fn is_archive(self) -> bool {
        self.contains(Self::ARCHIVE)
    }

    /// Returns `true` if the item is a reparse point (symlink, mount point, etc.).
    #[must_use]
    #[inline]
    pub fn is_reparse_point(self) -> bool {
        self.contains(Self::REPARSE_POINT)
    }

    /// Returns `true` if the item is stored compressed on disk.
    #[must_use]
    #[inline]
    pub fn is_compressed(self) -> bool {
        self.contains(Self::COMPRESSED)
    }

    /// Returns `true` if the item is stored encrypted on disk.
    #[must_use]
    #[inline]
    pub fn is_encrypted(self) -> bool {
        self.contains(Self::ENCRYPTED)
    }

    /// Returns `true` if the item contains sparse data.
    #[must_use]
    #[inline]
    pub fn is_sparse(self) -> bool {
        self.contains(Self::SPARSE_FILE)
    }

    /// Returns `true` if the item's data is not immediately available (offline).
    #[must_use]
    #[inline]
    pub fn is_offline(self) -> bool {
        self.contains(Self::OFFLINE)
    }

    /// Returns `true` if the item is marked temporary.
    #[must_use]
    #[inline]
    pub fn is_temporary(self) -> bool {
        self.contains(Self::TEMPORARY)
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

impl UsnReason {
    /// Every reason bit set — including bits this crate does not yet name.
    ///
    /// This is the recommended catch-all value for
    /// the Windows `JournalIterOptions` reason mask:
    /// unlike [`UsnReason::all`] (which only covers the flags defined above), it
    /// also matches reason bits introduced by newer Windows versions so no
    /// records are filtered out.
    pub const ALL: Self = Self::from_bits_retain(0xFFFF_FFFF);

    /// Returns `true` if this change created a file or directory.
    #[must_use]
    #[inline]
    pub fn is_file_create(self) -> bool {
        self.contains(Self::FILE_CREATE)
    }

    /// Returns `true` if this change deleted a file or directory.
    #[must_use]
    #[inline]
    pub fn is_file_delete(self) -> bool {
        self.contains(Self::FILE_DELETE)
    }

    /// Returns `true` if this record is part of a rename (either the old or the
    /// new name half of the operation).
    #[must_use]
    #[inline]
    pub fn is_rename(self) -> bool {
        self.intersects(Self::RENAME_OLD_NAME | Self::RENAME_NEW_NAME)
    }

    /// Returns `true` if the handle that caused the change was closed
    /// (the [`CLOSE`](Self::CLOSE) bit — the final record for a change set).
    #[must_use]
    #[inline]
    pub fn is_close(self) -> bool {
        self.contains(Self::CLOSE)
    }

    /// Returns `true` if any unnamed-stream data change occurred
    /// (overwrite, extend, or truncation).
    #[must_use]
    #[inline]
    pub fn is_data_change(self) -> bool {
        self.intersects(Self::DATA_OVERWRITE | Self::DATA_EXTEND | Self::DATA_TRUNCATION)
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
const SOURCE_INFO_NAMES: &[(u32, &str)] = &[
    (UsnSourceInfo::DATA_MANAGEMENT.bits(), "DATA_MANAGEMENT"),
    (UsnSourceInfo::AUXILIARY_DATA.bits(), "AUXILIARY_DATA"),
    (
        UsnSourceInfo::REPLICATION_MANAGEMENT.bits(),
        "REPLICATION_MANAGEMENT",
    ),
    (
        UsnSourceInfo::CLIENT_REPLICATION_MANAGEMENT.bits(),
        "CLIENT_REPLICATION_MANAGEMENT",
    ),
];

impl fmt::Display for UsnSourceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::display::write_flag_names(f, self.bits(), SOURCE_INFO_NAMES, " | ")
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
    fn usn_is_zero_and_arithmetic() {
        assert!(Usn::ZERO.is_zero());
        assert!(!Usn::new(1).is_zero());

        assert_eq!(Usn::new(10).saturating_add(5), Usn::new(15));
        assert_eq!(Usn::new(i64::MAX).saturating_add(1), Usn::new(i64::MAX));

        assert_eq!(Usn::new(10).checked_add(5), Some(Usn::new(15)));
        assert_eq!(Usn::new(i64::MAX).checked_add(1), None);
    }

    #[test]
    fn fid_hex_formatting_composes_with_specifiers() {
        let fid = Fid::new(0xDEAD);
        // Display keeps the 0x prefix.
        assert_eq!(format!("{fid}"), "0xdead");
        // Bare LowerHex / UpperHex, with the `#` flag adding the prefix.
        assert_eq!(format!("{fid:x}"), "dead");
        assert_eq!(format!("{fid:X}"), "DEAD");
        assert_eq!(format!("{fid:#x}"), "0xdead");
        assert_eq!(format!("{fid:#X}"), "0xDEAD");

        let ext = Fid::from_u128(0x1234_5678_9abc_def0);
        assert_eq!(format!("{ext:x}"), "123456789abcdef0");
    }

    #[test]
    fn file_attributes_display_is_none_for_zero_bits() {
        assert_eq!(format!("{}", FileAttributes::empty()), "NONE");
    }

    #[test]
    fn file_attributes_display_uses_hex_for_unknown_bits() {
        let attrs = FileAttributes::from_bits_retain(0x8000_0000);
        assert_eq!(format!("{attrs}"), "0x80000000");
    }

    #[test]
    fn usn_source_info_display_is_none_for_zero_bits() {
        assert_eq!(format!("{}", UsnSourceInfo::empty()), "NONE");
    }

    #[test]
    fn usn_source_info_display_uses_hex_for_unknown_bits() {
        let info = UsnSourceInfo::from_bits_retain(0x8000_0000);
        assert_eq!(format!("{info}"), "0x80000000");
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
    fn fid_from_parts_round_trips_record_and_sequence() {
        let fid = Fid::from_parts(0x2A, 0xABCD);
        assert!(fid.is_standard());
        assert_eq!(fid.record_number(), Some(0x2A));
        assert_eq!(fid.sequence(), Some(0xABCD));
        assert_eq!(fid.as_u64(), Some((0xABCDu64 << 48) | 0x2A));

        // Record numbers are masked to the low 48 bits.
        let masked = Fid::from_parts(u64::MAX, 0);
        assert_eq!(masked.record_number(), Some((1u64 << 48) - 1));
        assert_eq!(masked.sequence(), Some(0));
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

    #[test]
    fn standard_fid_from_u128_and_as_u128_zero_extends() {
        let fid = Fid::new(0x42);
        assert_eq!(fid.as_u128(), 0x42);
        assert_eq!(fid.as_bytes()[0], 0x42);
        assert!(fid.as_bytes()[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn fid_from_u128_is_extended() {
        let fid = Fid::from(0x1_0000_0000_0000_0001u128);
        assert!(fid.is_extended());
        assert_eq!(fid.as_u128(), 0x1_0000_0000_0000_0001u128);
        assert_eq!(fid.as_u64(), None);
    }

    #[test]
    fn usn_reason_all_is_full_mask() {
        assert_eq!(UsnReason::ALL.bits(), 0xFFFF_FFFF);
    }

    #[test]
    fn usn_reason_all_contains_every_named_flag() {
        // ALL must be a superset of bitflags' `all()` (the named flags) and then
        // some (the reserved/unknown bits), so newer reason bits still match.
        assert!(UsnReason::ALL.contains(UsnReason::all()));
        assert!(UsnReason::ALL.contains(UsnReason::FILE_CREATE | UsnReason::CLOSE));
    }

    #[test]
    fn usn_zero_const_is_zero() {
        assert_eq!(Usn::ZERO, Usn::new(0));
        assert_eq!(Usn::ZERO.get(), 0);
    }

    #[test]
    fn usn_reason_predicates() {
        assert!(UsnReason::FILE_CREATE.is_file_create());
        assert!(!UsnReason::FILE_DELETE.is_file_create());
        assert!(UsnReason::FILE_DELETE.is_file_delete());
        assert!(UsnReason::RENAME_OLD_NAME.is_rename());
        assert!(UsnReason::RENAME_NEW_NAME.is_rename());
        assert!(!UsnReason::FILE_CREATE.is_rename());
        assert!(UsnReason::CLOSE.is_close());
        assert!(UsnReason::DATA_EXTEND.is_data_change());
        assert!(UsnReason::DATA_TRUNCATION.is_data_change());
        assert!(!UsnReason::BASIC_INFO_CHANGE.is_data_change());
    }

    #[test]
    fn file_attributes_predicates() {
        assert!(FileAttributes::DIRECTORY.is_directory());
        assert!(FileAttributes::HIDDEN.is_hidden());
        assert!(FileAttributes::READ_ONLY.is_read_only());
        assert!(FileAttributes::SYSTEM.is_system());
        assert!(FileAttributes::ARCHIVE.is_archive());
        assert!(FileAttributes::REPARSE_POINT.is_reparse_point());
        assert!(FileAttributes::COMPRESSED.is_compressed());
        assert!(FileAttributes::ENCRYPTED.is_encrypted());
        assert!(FileAttributes::SPARSE_FILE.is_sparse());
        assert!(FileAttributes::OFFLINE.is_offline());
        assert!(FileAttributes::TEMPORARY.is_temporary());
        assert!(!FileAttributes::ARCHIVE.is_directory());

        let combined = FileAttributes::DIRECTORY | FileAttributes::HIDDEN;
        assert!(combined.is_directory());
        assert!(combined.is_hidden());
        assert!(!combined.is_read_only());
    }

    #[test]
    fn usn_source_info_display_lists_known_flags() {
        let info = UsnSourceInfo::DATA_MANAGEMENT | UsnSourceInfo::REPLICATION_MANAGEMENT;
        assert_eq!(info.to_string(), "DATA_MANAGEMENT | REPLICATION_MANAGEMENT");
    }

    #[test]
    fn file_attributes_display_lists_known_flags() {
        let attrs = FileAttributes::DIRECTORY | FileAttributes::HIDDEN;
        assert_eq!(attrs.to_string(), "HIDDEN | DIRECTORY");
    }
}
