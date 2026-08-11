//! Coalesced condition notification for an ISR-to-task handoff.
//!
//! [`EventFlags`] records whether each of exactly 32 payload-free conditions
//! occurred since the preceding take. The producer raises an [`EventMask`]
//! with one atomic `fetch_or`; the consumer takes every pending condition with
//! one atomic `swap(0)`. Repeated raises of the same condition may coalesce,
//! and conditions carry neither multiplicity nor ordering.
//!
//! A raise uses Release ordering and a take uses Acquire ordering. Therefore,
//! memory actions sequenced before a raise happen-before memory actions
//! sequenced after a take that observes that raise. The flags publish the fact
//! that application state is ready; they do not carry that state themselves.
//!
//! The handles are deliberately sole-role `Send + !Sync` values. An `&self`
//! hot-path receiver does not make a handle shareable: move each handle into
//! the one execution context that owns its role.

use core::cell::Cell;
use core::marker::PhantomData;
use core::ops::{BitAnd, BitOr, BitOrAssign};

use crate::sync::{AtomicBool, AtomicU32, Ordering};

/// A set of pending EventFlags conditions.
///
/// The representation is exactly one `u32`: bit indices 0 through 31 are the
/// complete condition namespace. Applications can define named `const` masks
/// with [`EventMask::from_bits`] without introducing a runtime mapping layer.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
#[must_use]
pub struct EventMask(u32);

impl EventMask {
    /// The empty condition set.
    pub const EMPTY: Self = Self(0);

    /// The set containing all 32 conditions.
    pub const ALL: Self = Self(u32::MAX);

    /// Construct a mask from its exact 32-bit representation.
    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Construct the one-condition mask at `index`.
    ///
    /// Returns `None` for an index outside `0..32`; no shift panic is
    /// reachable, including on a hot path that validates external input.
    #[inline(always)]
    #[must_use]
    pub const fn from_index(index: u32) -> Option<Self> {
        if index < u32::BITS {
            Some(Self(1u32 << index))
        } else {
            None
        }
    }

    /// Return the exact 32-bit representation.
    #[inline(always)]
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether the set contains no conditions.
    #[inline(always)]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every condition in `other` is present in this set.
    #[inline(always)]
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether this set and `other` share at least one condition.
    #[inline(always)]
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for EventMask {
    type Output = Self;

    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for EventMask {
    #[inline(always)]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for EventMask {
    type Output = Self;

    #[inline(always)]
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl core::fmt::Debug for EventMask {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "EventMask({:#010x})", self.0)
    }
}

/// A coalescing SPSC set of 32 payload-free conditions.
///
/// Exactly one [`Producer`] and one [`Consumer`] may be active at a time.
/// Acquisition is fallible and non-panicking; dropping a handle releases only
/// its role and leaves the pending set unchanged.
pub struct EventFlags {
    pending: AtomicU32,
    producer_taken: AtomicBool,
    consumer_taken: AtomicBool,
}

impl EventFlags {
    /// Create an empty EventFlags value.
    #[cfg(not(loom))]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            producer_taken: AtomicBool::new(false),
            consumer_taken: AtomicBool::new(false),
        }
    }

    /// Create an empty EventFlags value under Loom.
    #[cfg(loom)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
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
                flags: self,
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
                flags: self,
                _not_sync: PhantomData,
            })
        }
    }
}

impl Default for EventFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for EventFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventFlags")
            .field(
                "pending",
                &EventMask::from_bits(self.pending.load(Ordering::Relaxed)),
            )
            .finish_non_exhaustive()
    }
}

/// The sole raising handle for an [`EventFlags`] value.
///
/// This handle is `Send + !Sync`: it may move into an ISR or another execution
/// context, but it may not be shared between contexts.
///
/// ```compile_fail
/// use ph_eventing::event_flags::Producer;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<Producer<'static>>();
/// ```
pub struct Producer<'a> {
    flags: &'a EventFlags,
    _not_sync: PhantomData<Cell<()>>,
}

impl Producer<'_> {
    /// Raise every condition in `mask`.
    ///
    /// The operation is exactly one atomic `fetch_or`: it never loops, waits,
    /// allocates, calls user code, or panics. A concurrent take observes this
    /// raise in its own snapshot or leaves it pending for the following take.
    #[inline]
    pub fn raise(&self, mask: EventMask) {
        self.flags.pending.fetch_or(mask.bits(), Ordering::Release);
    }
}

impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.flags.producer_taken.store(false, Ordering::Release);
    }
}

impl core::fmt::Debug for Producer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("event_flags::Producer").finish()
    }
}

/// The sole taking handle for an [`EventFlags`] value.
///
/// This handle is `Send + !Sync` and may take pending conditions while its
/// paired producer raises them from another context.
///
/// ```compile_fail
/// use ph_eventing::event_flags::Consumer;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<Consumer<'static>>();
/// ```
pub struct Consumer<'a> {
    flags: &'a EventFlags,
    _not_sync: PhantomData<Cell<()>>,
}

impl Consumer<'_> {
    /// Atomically take every pending condition and clear the set.
    ///
    /// Each returned bit was raised at least once after the preceding take.
    /// Duplicate raises may coalesce and cross-condition order is not retained.
    #[inline]
    pub fn take_all(&self) -> EventMask {
        EventMask::from_bits(self.flags.pending.swap(0, Ordering::Acquire))
    }
}

impl Drop for Consumer<'_> {
    fn drop(&mut self) {
        self.flags.consumer_taken.store(false, Ordering::Release);
    }
}

impl core::fmt::Debug for Consumer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("event_flags::Consumer").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA_READY: EventMask = EventMask::from_bits(1 << 0);
    const OVERFLOW: EventMask = EventMask::from_bits(1 << 1);

    #[test]
    fn event_mask_is_an_explicit_panic_free_32_bit_set() {
        // Contract M1, R1, and W1-W2.
        assert_eq!(core::mem::size_of::<EventMask>(), 4);
        assert!(EventMask::EMPTY.is_empty());
        assert_eq!(EventMask::ALL.bits(), u32::MAX);
        assert_eq!(EventMask::from_index(0), Some(DATA_READY));
        assert_eq!(EventMask::from_index(31).unwrap().bits(), 1 << 31);
        assert_eq!(EventMask::from_index(32), None);
        assert_eq!(EventMask::from_index(u32::MAX), None);

        let both = DATA_READY | OVERFLOW;
        assert!(both.contains(DATA_READY));
        assert!(both.intersects(OVERFLOW));
        assert_eq!((both & OVERFLOW).bits(), OVERFLOW.bits());
    }

    #[test]
    fn duplicate_raises_coalesce_and_take_clears() {
        // Contract M1, R1-R2, T1-T2, C1, and C3.
        let flags = EventFlags::new();
        let producer = flags.try_producer().unwrap();
        let consumer = flags.try_consumer().unwrap();

        producer.raise(DATA_READY);
        producer.raise(DATA_READY);

        assert_eq!(consumer.take_all(), DATA_READY);
        assert_eq!(consumer.take_all(), EventMask::EMPTY);
    }

    #[test]
    fn multi_bit_and_all_bit_masks_round_trip() {
        // Contract R1, T1, C1, C3, and W1.
        let flags = EventFlags::new();
        let producer = flags.try_producer().unwrap();
        let consumer = flags.try_consumer().unwrap();

        producer.raise(DATA_READY | OVERFLOW);
        assert_eq!(consumer.take_all(), DATA_READY | OVERFLOW);
        producer.raise(EventMask::ALL);
        assert_eq!(consumer.take_all(), EventMask::ALL);
    }

    #[test]
    fn empty_raise_and_empty_take_are_no_ops() {
        // Contract R1 and T2.
        let flags = EventFlags::new();
        let producer = flags.try_producer().unwrap();
        let consumer = flags.try_consumer().unwrap();

        producer.raise(EventMask::EMPTY);
        assert!(consumer.take_all().is_empty());
    }

    #[test]
    fn handles_are_exclusive_and_reusable_after_drop() {
        // Contract H1 and H3.
        let flags = EventFlags::new();
        let producer = flags.try_producer().unwrap();
        let consumer = flags.try_consumer().unwrap();
        assert!(flags.try_producer().is_none());
        assert!(flags.try_consumer().is_none());

        producer.raise(DATA_READY);
        drop(producer);
        drop(consumer);

        let producer = flags.try_producer().expect("producer role released");
        let consumer = flags.try_consumer().expect("consumer role released");
        assert_eq!(consumer.take_all(), DATA_READY);
        producer.raise(OVERFLOW);
        assert_eq!(consumer.take_all(), OVERFLOW);
    }

    #[test]
    fn handles_are_send_and_container_is_sync() {
        // Contract H2. The compile-fail examples above pin `!Sync`.
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Producer<'static>>();
        assert_send::<Consumer<'static>>();
        assert_sync::<EventFlags>();
    }

    #[cfg(not(loom))]
    #[test]
    fn const_new_works_in_static_context() {
        // Contract H4.
        static FLAGS: EventFlags = EventFlags::new();
        let producer = FLAGS.try_producer().unwrap();
        let consumer = FLAGS.try_consumer().unwrap();
        producer.raise(DATA_READY);
        assert_eq!(consumer.take_all(), DATA_READY);
    }

    #[cfg(not(loom))]
    #[test]
    fn concurrent_raise_and_take_never_loses_the_condition() {
        // Contract C1-C3 at stress-test scale.
        use core::sync::atomic::{AtomicBool, Ordering as CoreOrdering};

        let flags = EventFlags::new();
        let producer = flags.try_producer().unwrap();
        let consumer = flags.try_consumer().unwrap();
        let done = AtomicBool::new(false);

        let seen = std::thread::scope(|scope| {
            let done_for_producer = &done;
            scope.spawn(move || {
                for _ in 0..crate::test_support::iterations(100_000) {
                    producer.raise(DATA_READY);
                }
                done_for_producer.store(true, CoreOrdering::Release);
            });

            let done_for_consumer = &done;
            let taker = scope.spawn(move || {
                let mut seen = EventMask::EMPTY;
                while !done_for_consumer.load(CoreOrdering::Acquire) {
                    seen |= consumer.take_all();
                    std::thread::yield_now();
                }
                seen | consumer.take_all()
            });

            taker.join().unwrap()
        });

        assert_eq!(seen, DATA_READY);
    }

    #[cfg(not(loom))]
    #[test]
    fn observed_raise_publishes_preceding_memory() {
        // Contract S1 at native/Miri scale; Loom supplies the weak-memory proof.
        use core::sync::atomic::{AtomicU32, Ordering as CoreOrdering};

        let flags = EventFlags::new();
        let producer = flags.try_producer().unwrap();
        let consumer = flags.try_consumer().unwrap();
        let payload = AtomicU32::new(0);

        std::thread::scope(|scope| {
            let payload_for_producer = &payload;
            scope.spawn(move || {
                payload_for_producer.store(0xA5A5_5A5A, CoreOrdering::Relaxed);
                producer.raise(DATA_READY);
            });

            while !consumer.take_all().contains(DATA_READY) {
                std::thread::yield_now();
            }
            assert_eq!(payload.load(CoreOrdering::Relaxed), 0xA5A5_5A5A);
        });
    }
}
