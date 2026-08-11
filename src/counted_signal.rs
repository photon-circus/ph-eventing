//! A saturating count for payload-free events.
//!
//! [`CountedSignal`] is an exploratory SPSC primitive for events whose
//! multiplicity matters but whose payload and ordering do not. Its producer
//! commits each increment with a sole-producer–bounded path; its consumer
//! atomically takes the accumulated count.
//!
//! The single-producer handle is load-bearing. Below [`u32::MAX`] the producer
//! uses a Relaxed `fetch_add`, which cannot wrap because only the consumer may
//! write between the load and the RMW and it can only reset to zero. An
//! observed `u32::MAX` is treated as maybe-stale and re-read through a no-op
//! RMW (`fetch_or(0)`), which — unlike a load — observes the latest value in
//! modification order: `MAX` confirms true saturation (skip), anything else
//! means a completed take reset the counter and the producer `fetch_add`s into
//! the new epoch. Every path is a fixed instruction sequence on every gated
//! ISA — no compare-exchange, so no LR/SC retry loop on RISC-V. Multiple
//! producers would invalidate the proof.
//!
//! The counter carries no payload-publication semantics. These Relaxed
//! operations order the count itself, but do not publish unrelated application
//! memory. Use a separate synchronization mechanism when an occurrence makes
//! payload data available.

use core::cell::Cell;
use core::marker::PhantomData;

use crate::sync::{AtomicBool, AtomicU32, Ordering};

/// A saturating SPSC counter for payload-free events.
///
/// Exactly one [`Producer`] and one [`Consumer`] may be active at a time.
/// Handles are `Send + !Sync`: each can move to another execution context,
/// but cannot be shared between contexts. Dropping a handle releases its slot.
///
/// `u32::MAX` is the saturation sentinel. Counts below it are exact; a
/// saturated snapshot means that at least `u32::MAX` increments occurred
/// since the preceding take.
pub struct CountedSignal {
    count: AtomicU32,
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

impl CountedSignal {
    /// Create an empty counted signal.
    #[cfg(not(loom))]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Create an empty counted signal under Loom.
    #[cfg(loom)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    #[cfg(all(loom, test))]
    pub(crate) fn with_count_for_model(count: u32) -> Self {
        Self {
            count: AtomicU32::new(count),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Probe-only seeding constructor. Not part of the public contract.
    ///
    /// The saturated `increment` arm is unreachable through the public API in
    /// bounded time — it needs the counter at `u32::MAX`, which is
    /// `u32::MAX` increments away — yet its cost is a measured claim
    /// (contract B1/B2). The QEMU cycle probe enables the hidden
    /// `_cycles-probe` feature to construct a saturated signal directly, the
    /// same way the Loom models seed epochs via `with_count_for_model`.
    #[cfg(feature = "_cycles-probe")]
    #[doc(hidden)]
    #[must_use]
    pub const fn with_count_for_probe(count: u32) -> Self {
        Self {
            count: AtomicU32::new(count),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Try to acquire the sole producer handle.
    ///
    /// Returns `None` while another producer handle is active.
    #[inline]
    pub fn try_producer(&self) -> Option<Producer<'_>> {
        if self.producer_taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Producer {
                signal: self,
                _not_sync: PhantomData,
            })
        }
    }

    /// Try to acquire the sole consumer handle.
    ///
    /// Returns `None` while another consumer handle is active.
    #[inline]
    pub fn try_consumer(&self) -> Option<Consumer<'_>> {
        if self.consumer_taken.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Consumer {
                signal: self,
                _not_sync: PhantomData,
            })
        }
    }
}

impl Default for CountedSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for CountedSignal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CountedSignal")
            .field("count", &self.count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// The sole incrementing handle for a [`CountedSignal`].
///
/// This handle is `Send + !Sync`. Its exclusivity is what keeps exact
/// saturation wrap-free with a fixed instruction sequence on every path —
/// the sentinel re-read is a no-op RMW, never a retry loop.
///
/// The load-bearing `!Sync` property is pinned at compile time:
///
/// ```compile_fail
/// use ph_eventing::counted_signal::Producer;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<Producer<'static>>();
/// ```
pub struct Producer<'a> {
    signal: &'a CountedSignal,
    _not_sync: PhantomData<Cell<()>>,
}

impl Producer<'_> {
    /// Record one occurrence.
    ///
    /// Below `u32::MAX` this is one Relaxed load and one Relaxed `fetch_add`.
    /// An observed `u32::MAX` is re-read through a no-op RMW (`fetch_or(0)`):
    /// `MAX` confirms saturation (skip), anything else is a post-take epoch
    /// and one `fetch_add` records the occurrence. Under sole-producer
    /// ownership the consumer's `swap(0)` is the only competing write, so
    /// every path is a fixed instruction sequence — no retry loop on any
    /// gated ISA — and the counter never wraps.
    #[inline]
    pub fn increment(&self) {
        // Only this handle may increase `count`; the consumer can only reset it
        // to zero. A plain skip on MAX can be stale after take returns, so the
        // sentinel path re-reads through an RMW: `fetch_or(0)` writes nothing
        // back but, unlike a load or a failed compare_exchange, is guaranteed
        // to observe the latest value in modification order — and it stays a
        // single atomic op on LR/SC ISAs, where a strong compare_exchange
        // lowers to a retry loop with no static bound (contract T3 / A1 / B1).
        let observed = self.signal.count.load(Ordering::Relaxed);
        if observed == u32::MAX && self.signal.count.fetch_or(0, Ordering::Relaxed) == u32::MAX {
            return;
        }
        // Wrap-free under H1: intervening writes can only lower the value.
        // Reached for every non-MAX observe, and for a stale MAX after take.
        self.signal.count.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.signal.producer_taken.store(false, Ordering::Release);
    }
}

impl core::fmt::Debug for Producer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("counted_signal::Producer").finish()
    }
}

/// The sole taking handle for a [`CountedSignal`].
///
/// This handle is `Send + !Sync` and may atomically take counts while its
/// paired producer increments from another context.
///
/// ```compile_fail
/// use ph_eventing::counted_signal::Consumer;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<Consumer<'static>>();
/// ```
pub struct Consumer<'a> {
    signal: &'a CountedSignal,
    _not_sync: PhantomData<Cell<()>>,
}

impl Consumer<'_> {
    /// Atomically take the count accumulated since the preceding take.
    ///
    /// A concurrent increment belongs wholly to this snapshot or wholly to
    /// the next one. [`CountSnapshot::is_saturated`] distinguishes the
    /// saturation sentinel from an exact count.
    #[inline]
    pub fn take_count(&self) -> CountSnapshot {
        CountSnapshot::from_raw(self.signal.count.swap(0, Ordering::Relaxed))
    }
}

impl Drop for Consumer<'_> {
    fn drop(&mut self) {
        self.signal.consumer_taken.store(false, Ordering::Release);
    }
}

impl core::fmt::Debug for Consumer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("counted_signal::Consumer").finish()
    }
}

/// The result of atomically taking a [`CountedSignal`] count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct CountSnapshot {
    count: u32,
}

impl CountSnapshot {
    #[inline(always)]
    const fn from_raw(raw: u32) -> Self {
        Self { count: raw }
    }

    /// Return the exact count, or `u32::MAX` when saturated.
    #[inline(always)]
    #[must_use]
    pub const fn count(self) -> u32 {
        self.count
    }

    /// Whether at least `u32::MAX` increments accumulated before the take.
    #[inline(always)]
    #[must_use]
    pub const fn is_saturated(self) -> bool {
        self.count == u32::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increments_accumulate_and_take_clears() {
        // Contract I1, T1, T4, and A1.
        let signal = CountedSignal::new();
        let producer = signal.try_producer().unwrap();
        let consumer = signal.try_consumer().unwrap();

        producer.increment();
        producer.increment();

        assert_eq!(consumer.take_count(), CountSnapshot { count: 2 });
        assert_eq!(consumer.take_count().count(), 0);
    }

    #[test]
    fn saturates_instead_of_wrapping() {
        // Contract I2-I3, T2, and A2-A3.
        let signal = CountedSignal::new();
        signal.count.store(u32::MAX - 1, Ordering::Relaxed);
        let producer = signal.try_producer().unwrap();
        let consumer = signal.try_consumer().unwrap();

        producer.increment();
        producer.increment();

        let snapshot = consumer.take_count();
        assert_eq!(snapshot.count(), u32::MAX);
        assert!(snapshot.is_saturated());
        assert_eq!(consumer.take_count().count(), 0);
    }

    #[test]
    fn handles_are_exclusive_and_reusable_after_drop() {
        // Contract H1 and H3: reacquisition continues existing state.
        let signal = CountedSignal::new();
        let producer = signal.try_producer().unwrap();
        let consumer = signal.try_consumer().unwrap();
        assert!(signal.try_producer().is_none());
        assert!(signal.try_consumer().is_none());

        producer.increment();

        drop(producer);
        drop(consumer);
        let producer = signal.try_producer().expect("producer role released");
        let consumer = signal.try_consumer().expect("consumer role released");
        assert_eq!(consumer.take_count().count(), 1);
        producer.increment();
        assert_eq!(consumer.take_count().count(), 1);
    }

    #[test]
    fn handles_are_send() {
        // Contract H2. The compile-fail examples above pin `!Sync`.
        fn assert_send<T: Send>() {}
        assert_send::<Producer<'static>>();
        assert_send::<Consumer<'static>>();
    }

    #[cfg(not(loom))]
    #[test]
    fn const_new_works_in_static_context() {
        // Contract H4.
        static SIGNAL: CountedSignal = CountedSignal::new();
        let producer = SIGNAL.try_producer().unwrap();
        let consumer = SIGNAL.try_consumer().unwrap();
        producer.increment();
        assert_eq!(consumer.take_count().count(), 1);
    }

    #[cfg(not(loom))]
    #[test]
    fn concurrent_takes_do_not_lose_increments() {
        // Contract T3 and A1 at stress-test scale.
        use core::sync::atomic::{AtomicBool, Ordering as CoreOrdering};

        let signal = CountedSignal::new();
        let producer = signal.try_producer().unwrap();
        let consumer = signal.try_consumer().unwrap();
        let done = AtomicBool::new(false);

        let total = std::thread::scope(|scope| {
            let done_for_producer = &done;
            scope.spawn(move || {
                for _ in 0..crate::test_support::iterations(100_000) {
                    producer.increment();
                }
                done_for_producer.store(true, CoreOrdering::Release);
            });

            let done_for_consumer = &done;
            let taker = scope.spawn(move || {
                let mut total = 0u64;
                while !done_for_consumer.load(CoreOrdering::Acquire) {
                    total += u64::from(consumer.take_count().count());
                    std::thread::yield_now();
                }
                total + u64::from(consumer.take_count().count())
            });

            taker.join().unwrap()
        });

        assert_eq!(total, u64::from(crate::test_support::iterations(100_000)));
    }
}
