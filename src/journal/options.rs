//! Options for iterating over the USN journal.

use crate::{Usn, UsnReason};
use std::num::NonZeroUsize;
use std::time::Duration;

use super::defaults::DEFAULT_BUFFER_BYTES_NONZERO;

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
    /// Kernel timeout, in whole seconds, for blocking reads (`wait_for_more`).
    /// `0` means block indefinitely until data is available.
    pub(crate) timeout_secs: u64,
    /// Whether the iterator should wait for more records.
    pub(crate) wait_for_more: bool,
    /// Size of the kernel output buffer.
    pub(crate) buffer_bytes: NonZeroUsize,
}

impl Default for JournalIterOptions {
    fn default() -> Self {
        JournalIterOptions {
            start_usn: Usn::new(0),
            reason_mask: UsnReason::ALL,
            only_on_close: false,
            timeout_secs: 0,
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

    /// Set the blocking-read timeout.
    ///
    /// Only meaningful together with [`Self::wait_for_more`]. The Win32 USN read
    /// API (`READ_USN_JOURNAL_DATA`) expresses this timeout in whole seconds, so
    /// the supplied [`Duration`] is truncated to `as_secs()`. The default —
    /// [`Duration::ZERO`] — blocks indefinitely until records are available.
    pub fn timeout(mut self, v: Duration) -> Self {
        self.inner.timeout_secs = v.as_secs();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_full_reason_mask() {
        assert_eq!(JournalIterOptions::default().reason_mask, UsnReason::ALL);
    }

    #[test]
    fn default_timeout_is_zero() {
        assert_eq!(JournalIterOptions::default().timeout_secs, 0);
    }

    #[test]
    fn timeout_truncates_duration_to_whole_seconds() {
        let opts = JournalIterOptions::builder()
            .timeout(Duration::from_millis(2_500))
            .build();
        assert_eq!(opts.timeout_secs, 2);
    }

    #[test]
    fn builder_round_trips_values() {
        let opts = JournalIterOptions::builder()
            .start_usn(Usn::new(42))
            .reason_mask(UsnReason::FILE_CREATE)
            .only_on_close(true)
            .wait_for_more(true)
            .timeout(Duration::from_secs(7))
            .buffer_bytes(NonZeroUsize::new(8 * 1024).unwrap())
            .build();

        assert_eq!(opts.start_usn, Usn::new(42));
        assert_eq!(opts.reason_mask, UsnReason::FILE_CREATE);
        assert!(opts.only_on_close);
        assert!(opts.wait_for_more);
        assert_eq!(opts.timeout_secs, 7);
        assert_eq!(opts.buffer_bytes.get(), 8 * 1024);
    }
}
