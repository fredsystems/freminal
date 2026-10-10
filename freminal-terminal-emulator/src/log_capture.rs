// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A minimal log-capture helper for tests that assert what a sequence handler
//! does and does not write to the log.
//!
//! Built on the `tracing` crate alone. One process-global [`Subscriber`] is
//! installed exactly once; it reports `Interest::always()` for every callsite
//! and routes each event to a thread-local sink, but only while [`capture`] is
//! active on the emitting thread. Events on any other thread are dropped.
//!
//! # Why a global subscriber
//!
//! `tracing-core` computes a callsite's interest from the *calling thread's*
//! default dispatcher on first hit whenever at most one dispatcher is
//! registered, and caches the answer process-wide. A per-thread
//! `with_default` subscriber therefore races with parallel tests: a thread
//! with no subscriber that first-hits a callsite caches `Interest::never`, and
//! the capturing thread's later events at that callsite are silently skipped.
//! A global subscriber that is always interested makes the cached interest
//! independent of which thread hits a callsite first.
//!
//! [`Subscriber`]: tracing::Subscriber

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};

/// One captured log event: its level and its formatted fields.
pub type Captured = (Level, String);

thread_local! {
    /// The active capture sink for this thread, if any.
    static SINK: RefCell<Option<Vec<Captured>>> = const { RefCell::new(None) };
}

static INSTALL: Once = Once::new();

struct Collector {
    next_span: AtomicU64,
}

struct FieldText(String);

impl Visit for FieldText {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        let _ = write!(self.0, "{}={value:?}", field.name());
    }
}

impl Subscriber for Collector {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        // `try_with` / `try_borrow_mut`: an event emitted during thread-local
        // teardown or re-entrantly from a field's `Debug` impl is dropped
        // rather than panicking.
        let _ = SINK.try_with(|sink| {
            if let Ok(mut sink) = sink.try_borrow_mut()
                && let Some(events) = sink.as_mut()
            {
                let mut text = FieldText(String::new());
                event.record(&mut text);
                events.push((*event.metadata().level(), text.0));
            }
        });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Install the process-global collector exactly once.
///
/// Panics (test-only code) if another global subscriber was already set in
/// this test binary, since capture could then silently see nothing.
fn install() {
    INSTALL.call_once(|| {
        let collector = Collector {
            next_span: AtomicU64::new(0),
        };
        assert!(
            tracing::subscriber::set_global_default(collector).is_ok(),
            "log_capture: a global tracing subscriber was already installed in this test binary"
        );
    });
}

/// Restores the previous thread-local sink when a [`capture`] scope ends,
/// including on unwind.
struct SinkGuard {
    previous: Option<Vec<Captured>>,
}

impl Drop for SinkGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = SINK.try_with(|sink| *sink.borrow_mut() = previous);
    }
}

/// Run `f` with every `tracing` event emitted on this thread captured, and
/// return the events in emission order.
pub fn capture(f: impl FnOnce()) -> Vec<Captured> {
    install();
    let previous = SINK.with(|sink| sink.borrow_mut().replace(Vec::new()));
    let guard = SinkGuard { previous };
    f();
    let events = SINK
        .with(|sink| sink.borrow_mut().take())
        .unwrap_or_default();
    drop(guard);
    events
}

/// The captured events at `warn` or `error` level.
pub fn warnings(events: &[Captured]) -> Vec<&Captured> {
    events
        .iter()
        .filter(|(level, _)| *level == Level::WARN || *level == Level::ERROR)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::{capture, warnings};

    /// A dedicated callsite: one `warn!` that nothing else in the test binary
    /// ever hits, so its first hit is controlled by the regression test.
    fn emit_probe() {
        tracing::warn!(probe = 1, "log_capture regression probe");
    }

    #[test]
    fn callsite_first_hit_by_uncaptured_thread_is_still_captured() {
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let events = capture(|| {
                scope.spawn(|| {
                    // Wait until the main thread is inside `capture`, then be
                    // the first to hit the callsite with no capture active here.
                    barrier.wait();
                    emit_probe();
                    barrier.wait();
                });
                barrier.wait();
                barrier.wait();
                emit_probe();
            });
            let warns = warnings(&events);
            assert_eq!(
                warns.len(),
                1,
                "expected exactly the capturing thread's event, got: {events:?}"
            );
        });
    }
}
