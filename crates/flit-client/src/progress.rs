//! Live progress for a send, readable while it is still running.
//!
//! A receiver that is fast enough finishes before a UI could draw a bar, and a
//! slow one needs to show that something is happening. The sender updates
//! atomics as bytes leave and the UI reads them on its own schedule: no lock,
//! no channel, and a reader can never slow the transfer down.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

/// How far along a send is. Cheap to clone; every clone reads the same counters.
#[derive(Debug, Clone, Default)]
pub struct SendProgress {
    inner: Arc<Counters>,
}

#[derive(Debug, Default)]
struct Counters {
    total_bytes: AtomicU64,
    sent_bytes: AtomicU64,
    total_files: AtomicUsize,
    files_done: AtomicUsize,
    cancelled: AtomicBool,
}

impl SendProgress {
    /// A fresh counter set, with no totals until the manifest exists.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what the whole send will cost, once every item is hashed.
    pub fn set_totals(&self, bytes: u64, files: usize) {
        self.inner.total_bytes.store(bytes, Ordering::Relaxed);
        self.inner.total_files.store(files, Ordering::Relaxed);
    }

    /// Total bytes the receiver is expected to hold when this finishes.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.inner.total_bytes.load(Ordering::Relaxed)
    }

    /// Bytes already handed to the network, or already held by the receiver
    /// from an earlier attempt.
    #[must_use]
    pub fn sent_bytes(&self) -> u64 {
        self.inner.sent_bytes.load(Ordering::Relaxed)
    }

    /// How many files the send covers.
    #[must_use]
    pub fn total_files(&self) -> usize {
        self.inner.total_files.load(Ordering::Relaxed)
    }

    /// How many files have a final result.
    #[must_use]
    pub fn files_done(&self) -> usize {
        self.inner.files_done.load(Ordering::Relaxed)
    }

    /// `0.0..=1.0`. A send with nothing in it reports `1.0`, never `NaN`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        let total = self.total_bytes();
        if total == 0 {
            return 1.0;
        }
        (self.sent_bytes() as f32 / total as f32).clamp(0.0, 1.0)
    }

    /// Count bytes that left this machine, or that were already on the receiver.
    pub fn add_bytes(&self, bytes: u64) {
        self.inner.sent_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Take bytes back when a resumed attempt turns out not to have kept them.
    ///
    /// Only the sender calls this, and only to make the count agree with what
    /// the receiver reports; it never goes below zero.
    pub(crate) fn subtract_bytes(&self, bytes: u64) {
        // A compare-exchange loop rather than `fetch_update`: the latter is
        // newer than this crate's MSRV.
        loop {
            let current = self.inner.sent_bytes.load(Ordering::Relaxed);
            let next = current.saturating_sub(bytes);
            if self
                .inner
                .sent_bytes
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Count one file as finished, whether it succeeded or failed.
    pub fn item_done(&self) {
        self.inner.files_done.fetch_add(1, Ordering::Relaxed);
    }

    /// Ask the sender to stop.
    ///
    /// Cooperative by design: the next chunk boundary notices, the request is
    /// abandoned, and the sender tells the receiver to drop the session. That
    /// is the difference between a cancel and a kill.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether [`SendProgress::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_counter_reports_nothing_sent_and_nothing_to_send() {
        let progress = SendProgress::new();
        assert_eq!(progress.fraction(), 1.0, "an empty send is not NaN");
        assert_eq!(progress.files_done(), 0);
        assert!(!progress.is_cancelled());
    }

    #[test]
    fn counters_moved_by_one_clone_are_seen_by_another() {
        let writer = SendProgress::new();
        let reader = writer.clone();
        writer.set_totals(1000, 4);
        writer.add_bytes(250);
        writer.item_done();

        assert_eq!(reader.total_bytes(), 1000);
        assert_eq!(reader.sent_bytes(), 250);
        assert_eq!(reader.files_done(), 1);
        assert_eq!(reader.total_files(), 4);
        assert!((reader.fraction() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn more_bytes_than_declared_never_passes_one() {
        let progress = SendProgress::new();
        progress.set_totals(10, 1);
        progress.add_bytes(99);
        assert_eq!(progress.fraction(), 1.0);
    }

    #[test]
    fn cancel_is_visible_to_the_sender() {
        let progress = SendProgress::new();
        let sender = progress.clone();
        assert!(!sender.is_cancelled());
        progress.cancel();
        assert!(sender.is_cancelled());
    }
}
