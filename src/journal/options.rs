//! Options for iterating over the USN journal.

use crate::{Usn, UsnReason};
use std::num::NonZeroUsize;

use super::defaults::{DEFAULT_BUFFER_BYTES_NONZERO, USN_REASON_MASK_ALL};

#[derive(Debug, Clone)]
/// Options for enumerating the USN journal.
///
/// Allows customization of the starting USN, reason mask, buffer size, and other parameters.
///
/// Use [`JournalIterOptions::builder`] for the fluent builder API.
pub struct JournalIterOptions {
    /// USN from which enumeration should begin.
    pub(crate) start_usn: Usn,
    /// Reason-mask filter applied by the kernel.
    pub(crate) reason_mask: UsnReason,
    /// Whether only close events should be returned.
    pub(crate) only_on_close: bool,
    /// Kernel timeout for blocking reads.
    pub(crate) timeout: u64,
    /// Whether the iterator should wait for more records.
    pub(crate) wait_for_more: bool,
    /// Size of the kernel output buffer.
    pub(crate) buffer_bytes: NonZeroUsize,
}

impl Default for JournalIterOptions {
    fn default() -> Self {
        JournalIterOptions {
            start_usn: Usn::new(0),
            reason_mask: UsnReason::from_bits_retain(USN_REASON_MASK_ALL),
            only_on_close: false,
            timeout: 0,
            wait_for_more: false,
            buffer_bytes: DEFAULT_BUFFER_BYTES_NONZERO,
        }
    }
}

impl JournalIterOptions {
    /// Returns a fluent builder for [`JournalIterOptions`].
    pub fn builder() -> JournalIterOptionsBuilder {
        JournalIterOptionsBuilder::default()
    }
}

/// Fluent builder for [`JournalIterOptions`].
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct JournalIterOptionsBuilder {
    /// Mutable options value being configured by the builder.
    inner: JournalIterOptions,
}

impl JournalIterOptionsBuilder {
    /// Set the starting USN.
    pub fn start_usn(mut self, v: Usn) -> Self {
        self.inner.start_usn = v;
        self
    }

    /// Set the reason mask filter.
    pub fn reason_mask(mut self, v: UsnReason) -> Self {
        self.inner.reason_mask = v;
        self
    }

    /// Only return records when the file is closed.
    pub fn only_on_close(mut self, v: bool) -> Self {
        self.inner.only_on_close = v;
        self
    }

    /// Set the timeout (Win32 USN read timeout, see `READ_USN_JOURNAL_DATA_V0`).
    pub fn timeout(mut self, v: u64) -> Self {
        self.inner.timeout = v;
        self
    }

    /// Whether the iterator should block waiting for more records.
    pub fn wait_for_more(mut self, v: bool) -> Self {
        self.inner.wait_for_more = v;
        self
    }

    /// Set the in-memory buffer size, in bytes.
    pub fn buffer_bytes(mut self, v: NonZeroUsize) -> Self {
        self.inner.buffer_bytes = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> JournalIterOptions {
        self.inner
    }
}
