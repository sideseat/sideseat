//! Turning per-message acknowledgement into a **contiguous offset** one backend can commit.
//!
//! Redis acknowledges an arbitrary id: `XACK` removes exactly the entry named and leaves its neighbours pending.
//! Kafka and RedPanda do not have that operation. They commit an *offset*, and committing offset N means "every
//! record up to N is done" — so committing the offset of a record that succeeded, while an **earlier** record in
//! the same partition failed, silently acknowledges the failure too. The failed record is then never redelivered,
//! and the data it carried is gone after the exporter was answered 200.
//!
//! That is the same class as the `MAXLEN` trim this repository already removed from the Redis publisher: a queue
//! that discards accepted work, invisibly, because the acknowledgement mechanism was taken at face value.
//!
//! So the adapter cannot pass an ack straight through. It has to track which offsets are complete and commit only
//! the **highest contiguous prefix** — everything below it is genuinely done. Records completed out of order wait,
//! held here, until the gap below them fills.
//!
//! Written and tested now, ahead of the RedPanda adapter, because the property is about the *contract* rather than
//! about any client library, and it is far easier to get right in isolation than inside a consumer loop.

use std::collections::BTreeSet;

/// The offsets a consumer has completed, and the prefix it may commit.
///
/// One per `(topic, partition, group)`: an offset means nothing across partitions, so mixing them would produce a
/// "contiguous" prefix that is not contiguous in any partition.
#[derive(Debug, Default)]
pub struct AckWindow {
    /// The next offset expected to complete. Everything below it is committed.
    next: u64,
    /// Completed offsets at or above `next`, waiting for the gap below them to fill.
    completed: BTreeSet<u64>,
}

impl AckWindow {
    /// A window starting at `first_offset`, which is the first offset this consumer was given.
    ///
    /// Not zero by default: a consumer resuming a partition starts wherever its group left off, and a window that
    /// assumed zero would report a committable prefix covering records it never saw.
    pub fn starting_at(first_offset: u64) -> Self {
        Self {
            next: first_offset,
            completed: BTreeSet::new(),
        }
    }

    /// Record that `offset` completed.
    ///
    /// Out of order is ordinary: a batch is processed concurrently, and one record finishing before an earlier one
    /// is the normal case rather than an error.
    pub fn complete(&mut self, offset: u64) {
        if offset < self.next {
            // Already covered by a commit. Recording it again is harmless and happens on a redelivery, which is
            // expected under at-least-once.
            return;
        }
        self.completed.insert(offset);
        while self.completed.remove(&self.next) {
            self.next += 1;
        }
    }

    /// The offset to commit, or `None` when nothing new is contiguous.
    ///
    /// This is the *exclusive* upper bound — the offset a consumer would next read — which is what Kafka's commit
    /// API takes. Returning the highest completed offset instead would commit one record too many, which is the
    /// silent loss this type exists to prevent.
    pub fn committable(&self, last_committed: Option<u64>) -> Option<u64> {
        match last_committed {
            Some(previous) if previous >= self.next => None,
            _ => Some(self.next),
        }
    }

    /// How many completed offsets are waiting on a gap below them.
    ///
    /// Exposed because it is the signal that something is stuck: a window that keeps growing means a record is
    /// failing repeatedly and everything behind it is un-committable. That is a condition to report, not to
    /// resolve by committing past it.
    pub fn held(&self) -> usize {
        self.completed.len()
    }

    /// The lowest offset not yet complete - the one holding the window open.
    pub fn blocked_on(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-order completion commits each record as it finishes.
    #[test]
    fn in_order_completion_advances_one_at_a_time() {
        let mut w = AckWindow::starting_at(10);
        assert_eq!(
            w.committable(None),
            Some(10),
            "nothing done yet, so the prefix is where we started"
        );

        w.complete(10);
        assert_eq!(w.committable(Some(10)), Some(11));
        w.complete(11);
        assert_eq!(w.committable(Some(11)), Some(12));
        assert_eq!(w.held(), 0);
    }

    /// **The property this type exists for**: a later record completing does not acknowledge an earlier failure.
    #[test]
    fn a_later_success_does_not_commit_past_an_earlier_failure() {
        let mut w = AckWindow::starting_at(0);
        // 0 is still being processed - or has failed. 1, 2 and 3 succeed.
        w.complete(1);
        w.complete(2);
        w.complete(3);

        assert_eq!(
            w.committable(None),
            Some(0),
            "committing anything above 0 would tell the broker that 0 is done, and it is not - that record is \
             then never redelivered and its data is gone after a 200"
        );
        assert_eq!(w.held(), 3, "three successes are waiting on the gap at 0");
        assert_eq!(w.blocked_on(), 0);

        // Once 0 finishes, the whole run becomes committable at once.
        w.complete(0);
        assert_eq!(w.committable(None), Some(4));
        assert_eq!(w.held(), 0);
    }

    /// A gap in the middle holds only what is above it.
    #[test]
    fn a_gap_in_the_middle_holds_only_what_is_above_it() {
        let mut w = AckWindow::starting_at(0);
        for offset in [0, 1, 3, 4] {
            w.complete(offset);
        }
        assert_eq!(w.committable(None), Some(2), "0 and 1 are done; 2 is not");
        assert_eq!(w.held(), 2, "3 and 4 wait");
        w.complete(2);
        assert_eq!(w.committable(None), Some(5));
    }

    /// A redelivery of something already committed changes nothing.
    #[test]
    fn re_completing_a_committed_offset_is_a_no_op() {
        let mut w = AckWindow::starting_at(0);
        w.complete(0);
        w.complete(1);
        assert_eq!(w.committable(None), Some(2));
        w.complete(0);
        assert_eq!(
            w.committable(None),
            Some(2),
            "at-least-once means this happens; it must not rewind"
        );
        assert_eq!(w.held(), 0);
    }

    /// Nothing to commit when the broker already has the prefix.
    #[test]
    fn nothing_is_committable_when_the_broker_is_current() {
        let mut w = AckWindow::starting_at(5);
        w.complete(5);
        assert_eq!(w.committable(Some(6)), None, "already committed through 6");
        assert_eq!(w.committable(Some(5)), Some(6));
    }

    /// A window resuming mid-partition does not claim records it never saw.
    #[test]
    fn a_resumed_window_does_not_claim_earlier_records() {
        let w = AckWindow::starting_at(1_000);
        assert_eq!(
            w.committable(None),
            Some(1_000),
            "a consumer resuming at 1000 must not report a prefix covering 0..1000, which it never processed"
        );
    }
}
