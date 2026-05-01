//! Options for iterating over the USN journal.

use crate::Usn;

use super::defaults::{DEFAULT_BUFFER_BYTES, USN_REASON_MASK_ALL};

#[derive(Debug, Clone)]
/// Options for enumerating the USN journal.
///
/// Allows customization of the starting USN, reason mask, buffer size, and other parameters.
///
/// Use [`JournalIterOptions::builder`] for the fluent builder API, or construct
/// directly via struct-literal syntax. [`Default`] is also implemented.
pub struct JournalIterOptions {
    pub start_usn: Usn,
    pub reason_mask: u32,
    pub only_on_close: bool,
    pub timeout: u64,
    pub wait_for_more: bool,
    pub buffer_size: usize,
}

impl Default for JournalIterOptions {
    fn default() -> Self {
        JournalIterOptions {
            start_usn: Usn::new(0),
            reason_mask: USN_REASON_MASK_ALL,
            only_on_close: false,
            timeout: 0,
            wait_for_more: false,
            buffer_size: DEFAULT_BUFFER_BYTES,
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
    inner: JournalIterOptions,
}

impl JournalIterOptionsBuilder {
    /// Set the starting USN.
    pub fn start_usn(mut self, v: Usn) -> Self {
        self.inner.start_usn = v;
        self
    }

    /// Set the reason mask filter.
    pub fn reason_mask(mut self, v: u32) -> Self {
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
    pub fn buffer_size(mut self, v: usize) -> Self {
        self.inner.buffer_size = v;
        self
    }

    /// Finalize the builder.
    #[must_use]
    pub fn build(self) -> JournalIterOptions {
        self.inner
    }
}
