//! The non-blocking offer to a bounded queue that the notification workers share.
//!
//! The macOS / Windows plain worker and the untracked OSC 99 worker both hand
//! work to a thread through a bounded queue and must never block the frame
//! thread. They differ in what they do about a full or gone queue, so this
//! module owns only the offer itself.

use crossbeam_channel::{Sender, TrySendError};

/// What a non-blocking offer to a bounded queue did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offered {
    /// The item is waiting for the consumer.
    Queued,
    /// The queue was full; the item was dropped.
    Full,
}

/// Offer `item` to `queue` without blocking. A disconnected queue hands the
/// item back so the caller can retry it elsewhere.
///
/// # Errors
///
/// Returns the item when the queue's receiver has been dropped.
pub fn offer<T>(queue: &Sender<T>, item: T) -> Result<Offered, T> {
    match queue.try_send(item) {
        Ok(()) => Ok(Offered::Queued),
        Err(TrySendError::Full(_)) => Ok(Offered::Full),
        Err(TrySendError::Disconnected(item)) => Err(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_up_to_capacity_then_reports_full() {
        let (tx, rx) = crossbeam_channel::bounded::<u32>(3);
        for i in 0..3 {
            assert_eq!(offer(&tx, i), Ok(Offered::Queued));
        }
        assert_eq!(offer(&tx, 3), Ok(Offered::Full));
        assert_eq!(offer(&tx, 4), Ok(Offered::Full));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(offer(&tx, 5), Ok(Offered::Queued));
    }

    #[test]
    fn a_disconnected_queue_hands_the_item_back() {
        let (tx, rx) = crossbeam_channel::bounded::<String>(3);
        drop(rx);
        assert_eq!(offer(&tx, "kept".to_owned()), Err("kept".to_owned()));
    }
}
